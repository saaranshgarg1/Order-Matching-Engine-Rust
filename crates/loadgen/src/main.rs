use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use clap::Parser;
use rand::{Rng, SeedableRng};
use rand::rngs::StdRng;
use tokio::io::{AsyncWriteExt, AsyncBufReadExt, BufReader};
use tokio::net::TcpStream;
use tracing::info;
use tracing_subscriber::EnvFilter;

#[derive(Parser, Debug)]
#[command(name = "loadgen", about = "Order book load generator")]
struct Args {
    #[arg(long, default_value = "127.0.0.1:9001")]
    addr: String,

    #[arg(long, default_value = "AAPL")]
    symbol: String,

    /// Orders per second per task (0 = unlimited)
    #[arg(long, default_value_t = 10_000)]
    rate: u64,

    /// Total orders to send (0 = unlimited)
    #[arg(long, default_value_t = 100_000)]
    count: u64,

    /// Parallel sender tasks
    #[arg(long, default_value_t = 4)]
    tasks: u64,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let args = Args::parse();
    info!("loadgen: {} total orders → {} at {}ops/task with {} tasks",
          args.count, args.addr, args.rate, args.tasks);

    let addr: SocketAddr = args.addr.parse().expect("invalid addr");
    let id_ctr   = Arc::new(AtomicU64::new(1));
    let sent_ctr = Arc::new(AtomicU64::new(0));
    let start    = Instant::now();

    let per_task = if args.count == 0 { u64::MAX } else { (args.count + args.tasks - 1) / args.tasks };

    let mut handles = vec![];
    for _ in 0..args.tasks {
        let id_ctr   = Arc::clone(&id_ctr);
        let sent_ctr = Arc::clone(&sent_ctr);
        let symbol   = args.symbol.clone();
        let rate     = args.rate;

        handles.push(tokio::spawn(async move {
            let stream = match TcpStream::connect(addr).await {
                Ok(s)  => s,
                Err(e) => { tracing::error!("connect: {e}"); return; }
            };
            let (read, mut write) = stream.into_split();
            // drain responses silently
            tokio::spawn(async move {
                let mut r = BufReader::new(read);
                let mut line = String::new();
                while r.read_line(&mut line).await.unwrap_or(0) > 0 { line.clear(); }
            });

            let interval = if rate > 0 {
                Some(Duration::from_nanos(1_000_000_000 / rate))
            } else { None };

            let mut rng = StdRng::from_entropy();
            let mut next_tick = Instant::now();

            for _ in 0..per_task {
                let id    = id_ctr.fetch_add(1, Ordering::Relaxed);
                let side  = if rng.gen_bool(0.5) { "buy" } else { "sell" };
                let price = 100.0 + rng.gen_range(-2.0f64..2.0);
                let qty   = rng.gen_range(1u64..=100);

                let msg = format!(
                    "{{\"t\":\"new\",\"id\":{id},\"symbol\":\"{symbol}\",\"side\":\"{side}\",\
                     \"type\":\"limit\",\"price\":{price:.2},\"qty\":{qty}}}\n"
                );
                if write.write_all(msg.as_bytes()).await.is_err() { break; }
                sent_ctr.fetch_add(1, Ordering::Relaxed);

                if let Some(iv) = interval {
                    next_tick += iv;
                    let now = Instant::now();
                    if now < next_tick { tokio::time::sleep(next_tick - now).await; }
                }
            }
        }));
    }

    // Progress printer
    let sc = Arc::clone(&sent_ctr);
    let total_target = args.count;
    tokio::spawn(async move {
        let t0 = Instant::now();
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            let n   = sc.load(Ordering::Relaxed);
            let ops = n as f64 / t0.elapsed().as_secs_f64();
            println!("sent={n}  rate={ops:.0}/s");
            if total_target > 0 && n >= total_target { break; }
        }
    });

    for h in handles { let _ = h.await; }

    let n       = sent_ctr.load(Ordering::Relaxed);
    let elapsed = start.elapsed().as_secs_f64();
    println!("\n=== done: {n} orders in {elapsed:.2}s = {:.0} ops/s ===",
             n as f64 / elapsed);
}
