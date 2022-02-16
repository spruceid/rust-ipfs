//! Construction of [Transports](`libp2p::Transport`) for IPFS.
//!
//! This module is only needed if you need to extend the functionality of the underlying
//! [Transport](`libp2p::Transport`).
//!
//! The base IPFS transport can be extended using [`TransportBuilder::or`] to wrap the base
//! transport implementation inside another (see [`OrTransport`]):
//!
//! ```
//! use ipfs::p2p::transport::{TTransport, TransportBuilder};
//! use libp2p::core::transport::MemoryTransport;
//! use libp2p::identity::Keypair;
//! let keypair: Keypair = Keypair::generate_ed25519();
//! let transport: TTransport = TransportBuilder::new(keypair).unwrap()
//!     .or(MemoryTransport::default())
//!     .build();
//! ```
//!
//! To perform additional [Upgrades](`Upgrade`) on the connection, first apply any Transport
//! extensions, use [`TransportBuilder::then`] to convert this builder into a [`TransportUpgrader`]
//! , and then [apply](`TransportUpgrader::apply`) upgrades:
//!
//! ```
//! use ipfs::p2p::transport::{TTransport, TransportBuilder};
//! use libp2p::core::upgrade;
//! use libp2p::identity::Keypair;
//! use std::io;
//! let keypair: Keypair = Keypair::generate_ed25519();
//! let upgrade = upgrade::from_fn("/foo/1", move |mut sock: upgrade::Negotiated<_>, endpoint| async move {
//!     if endpoint.is_dialer() {
//!         upgrade::write_length_prefixed(&mut sock, "some handshake data").await?;
//!         # use futures::AsyncWriteExt;
//!         sock.close().await?;
//!     } else {
//!         let handshake_data = upgrade::read_length_prefixed(&mut sock, 1024).await?;
//!         if handshake_data != b"some handshake data" {
//!             return Err(io::Error::new(io::ErrorKind::Other, "bad handshake"));
//!         }
//!     }
//!     Ok(sock)
//! });
//! let transport: TTransport = TransportBuilder::new(keypair).unwrap()
//!     .then()
//!     .apply(upgrade)
//!     .build();
//! ```
use futures::{AsyncRead, AsyncWrite, Future, TryFutureExt};
use libp2p::core::muxing::StreamMuxerBox;
use libp2p::core::transport::and_then::AndThen;
use libp2p::core::transport::upgrade::{Authenticate, Authenticated, Version};
use libp2p::core::transport::{Boxed, OrTransport, Upgrade};
use libp2p::core::upgrade::SelectUpgrade;
use libp2p::core::{ConnectedPoint, Negotiated, UpgradeInfo};
use libp2p::dns::{GenDnsConfig, TokioDnsConfig};
use libp2p::mplex::MplexConfig;
use libp2p::noise::{self, NoiseAuthenticated, NoiseConfig, X25519Spec, XX};
use libp2p::relay::v1::{Relay, RelayTransport};
use libp2p::tcp::tokio::Tcp;
use libp2p::tcp::{GenTcpConfig, TokioTcpConfig};
use libp2p::yamux::YamuxConfig;
use libp2p::{identity, InboundUpgrade, OutboundUpgrade};
use libp2p::{PeerId, Transport};
use std::error::Error as StdError;
use std::io::{self, Error, ErrorKind};
use std::time::Duration;
use trust_dns_resolver::name_server::{GenericConnection, GenericConnectionProvider, TokioRuntime};

/// Transport type.
pub type TTransport = Boxed<(PeerId, StreamMuxerBox)>;

pub fn default_transport(keypair: identity::Keypair) -> io::Result<TTransport> {
    TransportBuilder::new(keypair).map(TransportBuilder::build)
}

/// Builder for IPFS Transports.
///
/// This type can be used to build IPFS compatible Transport implementations. If you do not need to
/// extend the base IPFS transport implementation, then you do not need to use this builder and can
/// instead construct your [UninitializedIpfs](`crate::UninitializedIpfs`) directly from
/// [IpfsOptions](`crate::IpfsOptions`).
#[derive(Clone)]
pub struct TransportBuilder<T> {
    keypair: identity::Keypair,
    transport: T,
}

impl
    TransportBuilder<
        GenDnsConfig<GenTcpConfig<Tcp>, GenericConnection, GenericConnectionProvider<TokioRuntime>>,
    >
{
    pub fn new(keypair: identity::Keypair) -> io::Result<Self> {
        Ok(Self {
            keypair,
            transport: TokioDnsConfig::system(TokioTcpConfig::new())?,
        })
    }
}

impl<T> TransportBuilder<T>
where
    T: Transport + Clone + Send + Sync + 'static,
    T::Dial: Send,
    T::Error: Send + Sync + 'static,
    T::Listener: Send,
    T::ListenerUpgrade: Send,
    T::Output: AsyncWrite + AsyncRead + Unpin + Send + 'static,
{
    pub fn or<O: Transport>(self, other: O) -> TransportBuilder<OrTransport<O, T>> {
        TransportBuilder {
            keypair: self.keypair,
            transport: other.or_transport(self.transport),
        }
    }

    pub fn relay(self) -> (TransportBuilder<RelayTransport<T>>, Relay) {
        let (transport, relay) =
            libp2p::relay::v1::new_transport_and_behaviour(Default::default(), self.transport);
        (
            TransportBuilder {
                keypair: self.keypair,
                transport,
            },
            relay,
        )
    }

    pub fn map_auth(self) -> TransportAuthMapper<T, NoiseAuthenticated<XX, X25519Spec, ()>> {
        let xx_keypair = noise::Keypair::<noise::X25519Spec>::new()
            .into_authentic(&self.keypair)
            .unwrap();
        let noise_auth = NoiseConfig::xx(xx_keypair).into_authenticated();
        TransportAuthMapper {
            transport: self.transport,
            auth: noise_auth,
        }
    }

    pub fn apply_upgrades(
        self,
    ) -> TransportUpgrader<
        AndThen<
            T,
            impl Clone
                + FnOnce(
                    T::Output,
                    ConnectedPoint,
                )
                    -> Authenticate<T::Output, NoiseAuthenticated<XX, X25519Spec, ()>>,
        >,
    > {
        self.map_auth().apply_upgrades()
    }

    /// Builds the transport that serves as a common ground for all connections.
    ///
    /// Set up an encrypted TCP transport over the Mplex protocol.
    pub fn build(self) -> TTransport {
        self.map_auth().apply_upgrades().build()
    }
}

#[derive(Clone, Debug)]
pub struct TransportAuthMapper<T, A> {
    transport: T,
    auth: A,
}

impl<T, A, C, E, F> TransportAuthMapper<T, A>
where
    T: Transport + Clone + Send + Sync + 'static,
    T::Dial: Send,
    T::Error: Send + Sync + 'static,
    T::Listener: Send,
    T::ListenerUpgrade: Send,
    T::Output: AsyncWrite + AsyncRead + Unpin + Send + 'static,
    A: Clone + Send + Sync + 'static,
    A: InboundUpgrade<Negotiated<T::Output>, Output = (PeerId, C), Error = E, Future = F>,
    A: OutboundUpgrade<Negotiated<T::Output>, Output = (PeerId, C), Error = E, Future = F>,
    <A as UpgradeInfo>::Info: Send,
    <<A as UpgradeInfo>::InfoIter as IntoIterator>::IntoIter: Send,
    C: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    E: Send + Sync + StdError + 'static,
    F: Send + 'static,
{
    pub fn map_inbound<Fun>(
        self,
        fun: Fun,
    ) -> TransportAuthMapper<T, TryMapInboundUpgrade<A, Fun>>
where {
        let auth = TryMapInboundUpgrade {
            upgrade: self.auth,
            fun,
        };
        TransportAuthMapper {
            transport: self.transport,
            auth,
        }
    }

    pub fn map_outbound<Fun>(
        self,
        fun: Fun,
    ) -> TransportAuthMapper<T, TryMapOutboundUpgrade<A, Fun>>
where {
        let auth = TryMapOutboundUpgrade {
            upgrade: self.auth,
            fun,
        };
        TransportAuthMapper {
            transport: self.transport,
            auth,
        }
    }

    pub fn apply_upgrades(
        self,
    ) -> TransportUpgrader<
        AndThen<T, impl Clone + FnOnce(T::Output, ConnectedPoint) -> Authenticate<T::Output, A>>,
    > {
        TransportUpgrader {
            authenticated: self.transport.upgrade(Version::V1).authenticate(self.auth),
        }
    }

    /// Builds the transport that serves as a common ground for all connections.
    ///
    /// Set up an encrypted TCP transport over the Mplex protocol.
    pub fn build(self) -> TTransport {
        self.apply_upgrades().build()
    }
}

/// Upgrader for IPFS Transports.
///
/// Facilitates the application of [Upgrades](`Upgrade`) to the transport.
#[derive(Clone)]
pub struct TransportUpgrader<T> {
    authenticated: Authenticated<T>,
}

impl<T, C> TransportUpgrader<T>
where
    T: Transport<Output = (PeerId, C)> + Clone + Send + Sync + 'static,
    T::Dial: Send,
    T::Error: Send + Sync + 'static,
    T::Listener: Send,
    T::ListenerUpgrade: Send,
    C: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    pub fn apply<U, D, E, F>(self, u: U) -> TransportUpgrader<Upgrade<T, U>>
    where
        U: InboundUpgrade<Negotiated<C>, Output = D, Error = E, Future = F>,
        U: OutboundUpgrade<Negotiated<C>, Output = D, Error = E, Future = F>,
        U: Clone,
        D: AsyncRead + AsyncWrite + Unpin,
        E: StdError + 'static,
        F: Future,
    {
        let authenticated = self.authenticated.apply(u);
        TransportUpgrader { authenticated }
    }

    /// Builds the transport that serves as a common ground for all connections.
    ///
    /// Set up an encrypted TCP transport over the Mplex protocol.
    pub fn build(self) -> TTransport {
        self.authenticated
            .multiplex(SelectUpgrade::new(
                YamuxConfig::default(),
                MplexConfig::new(),
            ))
            .timeout(Duration::from_secs(20))
            .map(|(peer_id, muxer), _| (peer_id, StreamMuxerBox::new(muxer)))
            .map_err(|err| Error::new(ErrorKind::Other, err))
            .boxed()
    }
}

#[derive(Clone, Debug)]
pub struct TryMapInboundUpgrade<U, F> {
    upgrade: U,
    fun: F,
}

impl<U, F> UpgradeInfo for TryMapInboundUpgrade<U, F>
where
    U: UpgradeInfo,
{
    type Info = U::Info;

    type InfoIter = U::InfoIter;

    fn protocol_info(&self) -> Self::InfoIter {
        self.upgrade.protocol_info()
    }
}

impl<U, F, T, Fut, D> InboundUpgrade<T> for TryMapInboundUpgrade<U, F>
where
    U: InboundUpgrade<T>,
    F: FnOnce(U::Output) -> Fut,
    Fut: Future<Output = Result<D, U::Error>>,
{
    type Output = D;

    type Error = U::Error;

    type Future = futures::future::AndThen<U::Future, Fut, F>;

    fn upgrade_inbound(self, socket: T, info: Self::Info) -> Self::Future {
        self.upgrade
            .upgrade_inbound(socket, info)
            .and_then(self.fun)
    }
}

#[derive(Clone, Debug)]
pub struct TryMapOutboundUpgrade<U, F> {
    upgrade: U,
    fun: F,
}

impl<U, F> UpgradeInfo for TryMapOutboundUpgrade<U, F>
where
    U: UpgradeInfo,
{
    type Info = U::Info;

    type InfoIter = U::InfoIter;

    fn protocol_info(&self) -> Self::InfoIter {
        self.upgrade.protocol_info()
    }
}

impl<U, F, T, Fut, D> OutboundUpgrade<T> for TryMapOutboundUpgrade<U, F>
where
    U: OutboundUpgrade<T>,
    F: FnOnce(U::Output) -> Fut,
    Fut: Future<Output = Result<D, U::Error>>,
{
    type Output = D;

    type Error = U::Error;

    type Future = futures::future::AndThen<U::Future, Fut, F>;

    fn upgrade_outbound(self, socket: T, info: Self::Info) -> Self::Future {
        self.upgrade
            .upgrade_outbound(socket, info)
            .and_then(self.fun)
    }
}
