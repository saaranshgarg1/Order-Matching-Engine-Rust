use criterion::{black_box, criterion_group, criterion_main, Criterion, BenchmarkId};
use exchange_core::{OrderBook, Command, NewOrder, Sequenced, OrderType, TimeInForce, Side, apply};

fn make_book() -> OrderBook {
    OrderBook::new(1, 1)
}

fn seq(n: u64, cmd: Command) -> Sequenced {
    Sequenced { seq: n, ts: n * 100, cmd }
}

fn limit(id: u64, side: Side, price: i64, qty: u64) -> Command {
    Command::New(NewOrder {
        id, symbol: 1, side, kind: OrderType::Limit,
        tif: TimeInForce::Gtc, price, stop_price: 0, qty,
    })
}

fn bench_insert_no_cross(c: &mut Criterion) {
    c.bench_function("insert_1000_no_cross", |b| {
        b.iter(|| {
            let mut book = make_book();
            let mut out = Vec::with_capacity(8);
            for i in 1u64..=500 {
                out.clear();
                apply(&mut book, &seq(i, limit(i, Side::Buy, 100 - i as i64, 10)), &mut out);
            }
            for i in 1u64..=500 {
                out.clear();
                apply(&mut book, &seq(500+i, limit(500+i, Side::Sell, 101 + i as i64, 10)), &mut out);
            }
            black_box(&book);
        });
    });
}

fn bench_match_throughput(c: &mut Criterion) {
    let sizes = [100u64, 1_000, 10_000];
    let mut group = c.benchmark_group("match_throughput");
    for &n in &sizes {
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, &n| {
            b.iter(|| {
                let mut book = make_book();
                let mut out = Vec::with_capacity(8);
                // pre-load asks
                for i in 1..=n {
                    out.clear();
                    apply(&mut book, &seq(i, limit(i, Side::Sell, 100, 10)), &mut out);
                }
                // sweep with buys
                for i in 1..=n {
                    out.clear();
                    apply(&mut book, &seq(n+i, limit(n+i, Side::Buy, 100, 10)), &mut out);
                }
                black_box(&book);
            });
        });
    }
    group.finish();
}

fn bench_cancel(c: &mut Criterion) {
    c.bench_function("cancel_1000", |b| {
        b.iter(|| {
            let mut book = make_book();
            let mut out = Vec::with_capacity(4);
            for i in 1u64..=1000 {
                out.clear();
                apply(&mut book, &seq(i, limit(i, Side::Buy, 100 - (i % 50) as i64, 10)), &mut out);
            }
            for i in 1u64..=1000 {
                out.clear();
                apply(&mut book, &seq(1000+i, Command::Cancel { id: i, symbol: 1 }), &mut out);
            }
            black_box(&book);
        });
    });
}

criterion_group!(benches, bench_insert_no_cross, bench_match_throughput, bench_cancel);
criterion_main!(benches);
