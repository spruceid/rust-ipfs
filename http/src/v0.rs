use ipfs::{Ipfs, IpfsTypes};
use warp::{query, Filter};

pub mod bitswap;
pub mod block;
pub mod dag;
pub mod id;
pub mod pubsub;
pub mod refs;
pub mod root_files;
pub mod swarm;
pub mod version;

pub mod support;
pub use support::recover_as_message_response;
pub(crate) use support::{with_ipfs, InvalidPeerId, NotImplemented, StringError};

/// Helper to combine multiple filters together with Filter::or, possibly boxing the types in
/// the process. This greatly helps the build times for `ipfs-http`.
macro_rules! combine {
    ($x:expr, $($y:expr),+ $(,)?) => {
        {
            let filter = $x;
            $(
                let filter = filter.or($y);
            )+
            filter
        }
    }
}

/// Helper to combine multiple filters together with Filter::or. The filters are never boxed but
/// the output is assumed to be equal and so the output is unified.
macro_rules! combine_unify {
    ($x:expr, $($y:expr),+ $(,)?) => {
        {
            let filter = $x;
            $(
                let filter = filter.or($y).unify();
            )+
            filter
        }
    }
}

/// Macro will cause boxing on debug builds. Might be a good idea to explore how much boxing always
/// would speed up builds.
#[cfg(not(debug_assertions))]
macro_rules! boxed_on_debug {
    ($x:expr) => {
        $x
    };
}

#[cfg(debug_assertions)]
macro_rules! boxed_on_debug {
    ($x:expr) => {
        $x.boxed()
    };
}

/// Like `Filter::and` but the next filter is boxed. This might be a good idea to combine path
/// matching to the route implementation while maintaining a healthy balance for compilation time
/// and optimization.
macro_rules! and_boxed {
    ($x:expr, $y:expr) => {
        ($x).and(boxed_on_debug!($y))
    };
}

/// Supported routes of the crate.
pub fn routes<T: IpfsTypes>(
    ipfs: &Ipfs<T>,
    shutdown_tx: tokio::sync::mpsc::Sender<()>,
) -> impl warp::Filter<Extract = (impl warp::Reply,), Error = warp::Rejection> + Clone {
    let mount = warp::post().and(warp::path!("api" / "v0" / ..));

    let api = mount.and(combine!(
        and_boxed!(
            warp::path!("shutdown"),
            warp::any()
                .map(move || shutdown_tx.clone())
                .and_then(handle_shutdown)
        ),
        and_boxed!(warp::path!("id"), id::identity(ipfs)),
        and_boxed!(warp::path!("add"), root_files::add(ipfs)),
        and_boxed!(warp::path!("cat"), root_files::cat(ipfs)),
        and_boxed!(warp::path!("get"), root_files::get(ipfs)),
        and_boxed!(warp::path!("refs" / "local"), refs::local(ipfs)),
        and_boxed!(warp::path!("refs"), refs::refs(ipfs)),
        warp::path!("version")
            .and(query::<version::Query>())
            .and_then(version::version),
        warp::path("bitswap").and(combine!(
            and_boxed!(warp::path!("wantlist"), bitswap::wantlist(ipfs)),
            and_boxed!(warp::path!("stat"), bitswap::stat(ipfs))
        )),
        warp::path("block").and(combine!(
            and_boxed!(warp::path!("get"), block::get(ipfs)),
            and_boxed!(warp::path!("put"), block::put(ipfs)),
            and_boxed!(warp::path!("rm"), block::rm(ipfs)),
            and_boxed!(warp::path!("stat"), block::stat(ipfs)),
        )),
        warp::path("dag").and(combine!(
            and_boxed!(warp::path!("put"), dag::put(ipfs)),
            and_boxed!(warp::path!("resolve"), dag::resolve(ipfs)),
        )),
        warp::path("pubsub").and(combine!(
            and_boxed!(warp::path!("peers"), pubsub::peers(ipfs)),
            and_boxed!(warp::path!("ls"), pubsub::list_subscriptions(ipfs)),
            and_boxed!(warp::path!("pub"), pubsub::publish(ipfs)),
            and_boxed!(
                warp::path!("sub"),
                pubsub::subscribe(ipfs, Default::default())
            ),
        )),
        warp::path("swarm").and(combine!(
            and_boxed!(warp::path!("addrs" / "local"), swarm::addrs_local(ipfs)),
            and_boxed!(warp::path!("addrs"), swarm::addrs(ipfs)),
            and_boxed!(warp::path!("connect"), swarm::connect(ipfs)),
            and_boxed!(warp::path!("disconnect"), swarm::disconnect(ipfs)),
            and_boxed!(warp::path!("peers"), swarm::peers(ipfs)),
        )),
        combine_unify!(
            warp::path!("bootstrap" / ..),
            warp::path!("config" / ..),
            warp::path!("dht" / ..),
            warp::path!("key" / ..),
            warp::path!("name" / ..),
            warp::path!("object" / ..),
            warp::path!("pin" / ..),
            warp::path!("ping" / ..),
            warp::path!("repo" / ..),
            warp::path!("stats" / ..),
        )
        .and_then(not_implemented),
    ));

    // have a common handler turn the rejections into 400 or 500 with json body
    // boxing this might save up to 15s.
    boxed_on_debug!(api.recover(recover_as_message_response))
}

pub(crate) async fn handle_shutdown(
    mut tx: tokio::sync::mpsc::Sender<()>,
) -> Result<impl warp::Reply, std::convert::Infallible> {
    Ok(match tx.send(()).await {
        Ok(_) => warp::http::StatusCode::OK,
        Err(_) => warp::http::StatusCode::NOT_IMPLEMENTED,
    })
}

async fn not_implemented() -> Result<(impl warp::Reply,), std::convert::Infallible> {
    Ok((warp::http::StatusCode::NOT_IMPLEMENTED,))
}

#[cfg(test)]
mod tests {
    use ipfs::{Ipfs, TestTypes};
    /// Creates routes for tests, the ipfs will not work as no background task is being spawned.
    async fn testing_routes(
    ) -> impl warp::Filter<Extract = impl warp::Reply, Error = warp::Rejection> + Clone {
        use super::routes;
        use ipfs::{IpfsOptions, UninitializedIpfs};

        let options = IpfsOptions::inmemory_with_generated_keys();
        let (ipfs, _): (Ipfs<TestTypes>, _) = UninitializedIpfs::new(options, None)
            .await
            .start()
            .await
            .unwrap();

        let (shutdown_tx, _) = tokio::sync::mpsc::channel::<()>(1);

        routes(&ipfs, shutdown_tx)
    }

    #[tokio::test]
    async fn not_found_as_plaintext() {
        let routes = testing_routes().await;
        let resp = warp::test::request()
            .method("GET")
            .path("/api/v0/id_foobar")
            .reply(&routes)
            .await;

        assert_eq!(resp.status(), 404);
        // from go-ipfs
        assert_eq!(resp.body(), "404 page not found");
    }

    #[tokio::test]
    async fn invalid_peer_id_as_messageresponse() {
        let routes = testing_routes().await;
        let resp = warp::test::request()
            .method("POST")
            .path("/api/v0/id?arg=foobar")
            .reply(&routes)
            .await;

        assert_eq!(resp.status(), 400);
        // from go-ipfs
        assert_eq!(
            resp.body(),
            r#"{"Message":"invalid peer id","Code":0,"Type":"error"}"#
        );
    }
}
