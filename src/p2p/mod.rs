//! P2P handling for IPFS nodes.
use crate::{IpfsOptions, IpfsTypes};
use libp2p::identity::Keypair;
use libp2p::swarm::NetworkBehaviour;
use libp2p::Swarm;
use libp2p::{Multiaddr, PeerId};
use std::io;
use tracing::Span;

pub(crate) mod addr;
mod behaviour;
pub(crate) mod pubsub;
mod swarm;
pub mod transport;

pub use addr::{MultiaddrWithPeerId, MultiaddrWithoutPeerId};

pub use {
    behaviour::{Behaviour, CustomBehaviourBuilder, KadResult, NoopBehaviour},
    swarm::Connection,
};

/// Type alias for [`libp2p::Swarm`] running the [`behaviour::Behaviour`] with the given [`IpfsTypes`].
pub type TSwarm<T> = Swarm<Behaviour<T, NoopBehaviour>>;
/// Type alias for [`libp2p::Swarm`] running the [`behaviour::Behaviour`] with the given [`IpfsTypes`],
/// and a custom [`libp2p::swarm::NetworkBehaviour`].
pub type TCustomSwarm<T, Custom> = Swarm<Behaviour<T, Custom>>;

/// Defines the configuration for an IPFS swarm.
pub struct SwarmOptions {
    /// The keypair for the PKI based identity of the local node.
    pub keypair: Keypair,
    /// The peer address of the local node created from the keypair.
    pub peer_id: PeerId,
    /// The peers to connect to on startup.
    pub bootstrap: Vec<(Multiaddr, PeerId)>,
    /// Enables mdns for peer discovery and announcement when true.
    pub mdns: bool,
    /// Custom Kademlia protocol name, see [`IpfsOptions::kad_protocol`].
    pub kad_protocol: Option<String>,
}

impl From<&IpfsOptions> for SwarmOptions {
    fn from(options: &IpfsOptions) -> Self {
        let keypair = options.keypair.clone();
        let peer_id = keypair.public().into_peer_id();
        let bootstrap = options.bootstrap.clone();
        let mdns = options.mdns;
        let kad_protocol = options.kad_protocol.clone();

        SwarmOptions {
            keypair,
            peer_id,
            bootstrap,
            mdns,
            kad_protocol,
        }
    }
}

/// Creates a new IPFS swarm.
pub fn create_swarm<TIpfsTypes: IpfsTypes, Custom: NetworkBehaviour<OutEvent = ()>>(
    options: SwarmOptions,
    span: Span,
    behaviour: Behaviour<TIpfsTypes, Custom>,
) -> io::Result<TCustomSwarm<TIpfsTypes, Custom>> {
    let peer_id = options.peer_id;

    // Set up an encrypted TCP transport over the Mplex protocol.
    let transport = transport::TransportBuilder::new(options.keypair.clone())?.build_transport();
    //let upgrader = transport::TransportBuilder::new(options.keypair.clone())?.then();
    //use crate::apply_upgrades;
    //let transport = apply_upgrades!(upgrader =>);

    // Create a Swarm
    let swarm = libp2p::swarm::SwarmBuilder::new(transport, behaviour, peer_id)
        .executor(Box::new(SpannedExecutor(span)))
        .build();

    Ok(swarm)
}

struct SpannedExecutor(Span);

impl libp2p::core::Executor for SpannedExecutor {
    fn exec(
        &self,
        future: std::pin::Pin<Box<dyn std::future::Future<Output = ()> + 'static + Send>>,
    ) {
        use tracing_futures::Instrument;
        tokio::task::spawn(future.instrument(self.0.clone()));
    }
}
