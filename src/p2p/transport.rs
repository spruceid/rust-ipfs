use futures::{AsyncRead, AsyncWrite, Future};
use libp2p::core::muxing::StreamMuxerBox;
use libp2p::core::transport::upgrade::{Builder, Version};
use libp2p::core::transport::{Boxed, OrTransport};
use libp2p::core::upgrade::SelectUpgrade;
use libp2p::core::{Negotiated, UpgradeInfo};
use libp2p::dns::{GenDnsConfig, TokioDnsConfig};
use libp2p::mplex::MplexConfig;
use libp2p::noise::{self, NoiseAuthenticated, NoiseConfig, NoiseOutput, X25519Spec, XX};
use libp2p::relay::{Relay, RelayTransport};
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
pub(crate) type TTransport = Boxed<(PeerId, StreamMuxerBox)>;

pub fn default_transport(keypair: identity::Keypair) -> io::Result<TTransport> {
    TransportBuilder::new(keypair).map(TransportBuilder::build_transport)
}
pub struct TransportBuilder<T: Transport> {
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

impl<T: Transport> TransportBuilder<T>
where
    <T as Transport>::Error: 'static,
{
    pub fn or<O: Transport>(self, other: O) -> TransportBuilder<OrTransport<O, T>> {
        TransportBuilder {
            keypair: self.keypair,
            transport: other.or_transport(self.transport),
        }
    }
}

impl<T: Transport + Clone> TransportBuilder<T> {
    pub fn relay(self) -> (TransportBuilder<RelayTransport<T>>, Relay) {
        let (transport, relay) =
            libp2p::relay::new_transport_and_behaviour(Default::default(), self.transport);
        (
            TransportBuilder {
                keypair: self.keypair,
                transport,
            },
            relay,
        )
    }
}

impl<T: Transport + Clone + Send + Sync + 'static> TransportBuilder<T>
where
    <T as Transport>::Error: Send + Sync + 'static,
    <T as Transport>::Output: AsyncWrite + AsyncRead + Unpin + Send + 'static,
    <T as Transport>::Listener: Send,
    <T as Transport>::ListenerUpgrade: Send,
    <T as Transport>::Dial: Send,
{
    /// Builds the transport that serves as a common ground for all connections.
    ///
    /// Set up an encrypted TCP transport over the Mplex protocol.
    pub fn build_transport(self) -> TTransport {
        let xx_keypair = noise::Keypair::<noise::X25519Spec>::new()
            .into_authentic(&self.keypair)
            .unwrap();
        let noise_config = NoiseConfig::xx(xx_keypair).into_authenticated();
        self.transport
>>>>>>> ce11eca (Make the ipfs transport extensible.)
            .upgrade(Version::V1)
            .authenticate(noise_config)
            .multiplex(SelectUpgrade::new(
                YamuxConfig::default(),
                MplexConfig::new(),
            ))
            .timeout(Duration::from_secs(20))
            .map(|(peer_id, muxer), _| (peer_id, StreamMuxerBox::new(muxer)))
            .map_err(|err| Error::new(ErrorKind::Other, err))
            .boxed()
    }

    /// Builds the transport that serves as a common ground for all connections.
    ///
    /// Set up an encrypted TCP transport over the Mplex protocol.
    pub fn build_transport_with_upgrade<U, D, E, F, I, It>(self, upgrade: U) -> TTransport
    where
        D: AsyncRead + AsyncWrite + Unpin + Send + 'static,
        E: StdError + Send + Sync + 'static,
        F: Future + Send + Sync,
        I: IntoIterator<IntoIter = It>,
        It: Send + Sync + Iterator,
        <It as Iterator>::Item: Send + Sync,
        U: Send + Sync + Clone + 'static,
        U: InboundUpgrade<
            Negotiated<NoiseOutput<Negotiated<<T as Transport>::Output>>>,
            Output = D,
            Error = E,
            Future = F,
        >,
        U: OutboundUpgrade<
            Negotiated<NoiseOutput<Negotiated<<T as Transport>::Output>>>,
            Output = D,
            Error = E,
            Future = F,
        >,
        U: UpgradeInfo<InfoIter = I>,
        <U as UpgradeInfo>::Info: Send,
    {
        let xx_keypair = noise::Keypair::<noise::X25519Spec>::new()
            .into_authentic(&self.keypair)
            .unwrap();
        let noise_config = NoiseConfig::xx(xx_keypair).into_authenticated();
        self.transport
            .upgrade(Version::V1)
            .authenticate(noise_config)
            .apply(upgrade)
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

pub mod apply_upgrades {
    use super::{
        noise, Builder, NoiseAuthenticated, NoiseConfig, TransportBuilder, X25519Spec, XX,
    };

    pub use std::{
        io::{Error, ErrorKind},
        time::Duration,
    };

    pub use libp2p::{
        core::{
            muxing::StreamMuxerBox,
            upgrade::{SelectUpgrade, Version},
        },
        mplex::MplexConfig,
        yamux::YamuxConfig,
        Transport,
    };

    impl<T: Transport> TransportBuilder<T>
    where
        <T as Transport>::Error: 'static,
    {
        pub fn then(self) -> TransportUpgrader<T> {
            let xx_keypair = noise::Keypair::<noise::X25519Spec>::new()
                .into_authentic(&self.keypair)
                .unwrap();
            let noise_config = NoiseConfig::xx(xx_keypair).into_authenticated();
            TransportUpgrader {
                noise_config,
                builder: self.transport.upgrade(Version::V1),
            }
        }
    }

    pub struct TransportUpgrader<T: Transport> {
        pub noise_config: NoiseAuthenticated<XX, X25519Spec, ()>,
        pub builder: Builder<T>,
    }

    #[macro_export]
    macro_rules! apply_upgrades {
        ($upgrader:expr => $($upgrades:expr),*) => {
            {
                use $crate::p2p::transport::apply_upgrades::*;
                $upgrader.builder
                    .authenticate($upgrader.noise_config)
                    $(
                        .apply($upgrades)
                    )*
                    .multiplex(SelectUpgrade::new(
                        YamuxConfig::default(),
                        MplexConfig::new(),
                    ))
                    .timeout(Duration::from_secs(20))
                    .map(|(peer_id, muxer), _| (peer_id, StreamMuxerBox::new(muxer)))
                    .map_err(|err| Error::new(ErrorKind::Other, err))
                    .boxed()
            }
        };
    }
}
