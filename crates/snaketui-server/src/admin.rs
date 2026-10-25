//! A small HTTP port for load balancers and monitoring, kept off the
//! players' port: `/healthz` says whether to send this server players, and
//! `/metrics` has numbers for Prometheus.

use std::fmt::Write as _;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tracing::debug;

use crate::accept::Limits;
use crate::metrics::{PINGS, PingHistogram, TICK_LAG};
use crate::{Shared, Stopping};

/// A request has this long to arrive whole.
const READ_LIMIT: Duration = Duration::from_secs(2);
const MAX_REQUEST: usize = 4096;
/// Bucket bounds for the tick-lag histogram, in seconds.
const LAG_BOUNDS: [f64; 7] = [0.0005, 0.001, 0.002, 0.005, 0.01, 0.02, 0.05];

/// Answers until the task is dropped.
pub async fn serve_admin(listener: TcpListener, shared: Arc<Shared>, limits: Arc<Limits>) {
    loop {
        let Ok((stream, peer)) = listener.accept().await else {
            tokio::time::sleep(Duration::from_millis(100)).await;
            continue;
        };
        let (shared, limits) = (Arc::clone(&shared), Arc::clone(&limits));
        tokio::spawn(async move {
            if let Err(e) = answer(stream, &shared, &limits).await {
                debug!(%peer, error = %e, "an admin request failed");
            }
        });
    }
}

async fn answer(mut stream: TcpStream, shared: &Shared, limits: &Limits) -> std::io::Result<()> {
    let mut request = Vec::with_capacity(512);
    let mut chunk = [0; 512];
    let read = tokio::time::timeout(READ_LIMIT, async {
        while !request.windows(4).any(|w| w == b"\r\n\r\n") && request.len() < MAX_REQUEST {
            let n = stream.read(&mut chunk).await?;
            if n == 0 {
                break;
            }
            request.extend_from_slice(&chunk[..n]);
        }
        std::io::Result::Ok(())
    })
    .await;
    if !matches!(read, Ok(Ok(()))) {
        return Ok(());
    }
    let line = String::from_utf8_lossy(&request);
    let mut words = line.split_whitespace();
    let (method, path) = (words.next(), words.next());
    let (status, body) = match (method, path) {
        (Some("GET"), Some("/healthz")) => match *shared.shutdown.borrow() {
            Stopping::No => ("200 OK", "serving\n".to_string()),
            _ => ("503 Service Unavailable", "draining\n".to_string()),
        },
        (Some("GET"), Some("/metrics")) => ("200 OK", metrics(shared, limits)),
        _ => ("404 Not Found", "not found\n".to_string()),
    };
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/plain; version=0.0.4\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes()).await?;
    stream.shutdown().await
}

/// Prometheus's text format.
fn metrics(shared: &Shared, limits: &Limits) -> String {
    let mut out = String::new();
    let mut gauge = |name: &str, help: &str, value: f64| {
        let _ = writeln!(
            out,
            "# HELP {name} {help}\n# TYPE {name} gauge\n{name} {value}"
        );
    };
    gauge(
        "snaketui_players",
        "Players connected to this server.",
        limits.open() as f64,
    );
    gauge(
        "snaketui_rooms",
        "Rooms running on this server.",
        shared.rooms.len() as f64,
    );
    gauge(
        "snaketui_queue_waiting",
        "Players waiting for a quick-play match (the whole cluster's, in a cluster).",
        *shared.queue_len.borrow() as f64,
    );
    let draining = *shared.shutdown.borrow() != Stopping::No;
    gauge(
        "snaketui_draining",
        "1 while the server is stopping.",
        f64::from(u8::from(draining)),
    );
    if let Some(cluster) = &shared.cluster {
        gauge(
            "snaketui_cluster_peers",
            "Other servers of the cluster heard from lately.",
            cluster.peers().len() as f64,
        );
    }
    histogram(
        &mut out,
        "snaketui_tick_lag_seconds",
        "How late rooms' ticks ran.",
        LAG_BOUNDS.map(Duration::from_secs_f64),
        |bound| TICK_LAG.at_or_under(bound),
        (TICK_LAG.count(), TICK_LAG.sum()),
    );
    histogram(
        &mut out,
        "snaketui_ping_seconds",
        "Players' round trips to this server, measured every few seconds.",
        PingHistogram::BOUNDS,
        |bound| PINGS.at_or_under(bound),
        (PINGS.count(), PINGS.sum()),
    );
    out
}

/// A histogram in Prometheus's text format: how many samples were at or
/// under each bound, then their count and sum.
fn histogram<const N: usize>(
    out: &mut String,
    name: &str,
    help: &str,
    bounds: [Duration; N],
    at_or_under: impl Fn(Duration) -> u64,
    (count, sum): (u64, Duration),
) {
    let _ = writeln!(out, "# HELP {name} {help}\n# TYPE {name} histogram");
    for bound in bounds {
        let le = bound.as_secs_f64();
        let _ = writeln!(out, "{name}_bucket{{le=\"{le}\"}} {}", at_or_under(bound));
    }
    let _ = writeln!(out, "{name}_bucket{{le=\"+Inf\"}} {count}");
    let _ = writeln!(out, "{name}_sum {}", sum.as_secs_f64());
    let _ = writeln!(out, "{name}_count {count}");
}
