use axum::{
    body::StreamBody,
    extract::{BodyStream, Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, head, put},
    Router,
};
use clap::Parser;
use futures::TryStreamExt;
use std::{
    fs, io,
    net::{IpAddr, SocketAddr},
    path::PathBuf,
    sync::Arc,
};
use tokio::{fs::File, io::BufWriter};
use tokio_util::io::{ReaderStream, StreamReader};
use tower_http::trace::TraceLayer;
use tracing::info;

#[derive(clap::Parser)]
struct Args {
    #[clap(long)]
    root: PathBuf,

    #[clap(long, default_value = "3000")]
    port: u16,

    #[clap(long, default_value = "127.0.0.1")]
    local_addr: IpAddr,
}

#[tokio::main]
async fn main() {
    // initialize tracing
    tracing_subscriber::fmt::init();

    let args = Arc::new(Args::parse());

    // build our application with a route
    let app = Router::new()
        .route("/cache/:hash", get(cache_get))
        .route("/status", get(|| async { "online" }))
        .route("/cache/:hash", head(cache_head))
        .route("/cache/:hash", put(cache_put))
        .layer(TraceLayer::new_for_http())
        .with_state(args.clone());

    // run our app with hyper
    // `axum::Server` is a re-export of `hyper::Server`
    let addr = SocketAddr::from((args.local_addr, args.port));
    tracing::debug!("listening on {}", addr);
    axum::Server::bind(&addr)
        .serve(app.into_make_service())
        .await
        .unwrap();
}

fn hash_to_file(root: &std::path::Path, hash: &str) -> Result<PathBuf, (StatusCode, String)> {
    if hash.len() < 20 {
        return Err((StatusCode::BAD_REQUEST, "hash too short".into()));
    }

    let mut cache_path = root.to_owned();
    cache_path.push(&hash[..2]);
    cache_path.push(hash.to_owned() + ".zip");

    Ok(cache_path)
}

async fn cache_get(
    State(state): State<Arc<Args>>,
    Path(hash): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let cache_path = hash_to_file(&state.root, &hash)?;

    if !cache_path.exists() {
        return Err((
            StatusCode::NOT_FOUND,
            format!("{} does not exist", cache_path.display()),
        ));
    }

    Ok(StreamBody::new(ReaderStream::new(
        tokio::fs::File::open(cache_path)
            .await
            .map_err(|e| (StatusCode::NOT_FOUND, e.to_string()))?,
    )))
}

async fn cache_head(
    State(state): State<Arc<Args>>,
    Path(hash): Path<String>,
) -> Result<(), (StatusCode, String)> {
    let cache_path = hash_to_file(&state.root, &hash)?;

    if !cache_path.exists() {
        return Err((
            StatusCode::NOT_FOUND,
            format!("{} does not exist", cache_path.display()),
        ));
    }

    Ok(())
}

async fn cache_put(
    State(state): State<Arc<Args>>,
    Path(hash): Path<String>,
    body: BodyStream,
) -> Result<(), (StatusCode, String)> {
    let cache_path = hash_to_file(&state.root, &hash)?;

    if !cache_path.parent().unwrap().exists() {
        fs::create_dir(cache_path.parent().unwrap())
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    }

    let mut file = BufWriter::new(
        File::create(&cache_path)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?,
    );

    let body_with_io_error = body.map_err(|err| io::Error::new(io::ErrorKind::Other, err));
    let body_reader = StreamReader::new(body_with_io_error);
    futures::pin_mut!(body_reader);

    let bytes = tokio::io::copy(&mut body_reader, &mut file)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    info!(
        "Wrote {} to {}",
        human_bytes::human_bytes(bytes as f64),
        cache_path.display()
    );

    Ok(())
}
