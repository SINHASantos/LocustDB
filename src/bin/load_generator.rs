use std::time::Duration;

use locustdb::logging_client::BufferFullPolicy;
use locustdb_serialization::api::any_val_syntax::vf64;
use structopt::StructOpt;
use tokio::time;

#[derive(StructOpt, Debug)]
#[structopt(
    name = "LocustDB Logger Test",
    about = "Log basic system stats to LocustDB.",
    author = "Clemens Winter <clemenswinter1@gmail.com>"
)]
struct Opt {
    /// Address of LocustDB server
    #[structopt(long, name = "ADDR", default_value = "http://localhost:8080")]
    addr: String,

    /// Logging interval in milliseconds
    #[structopt(long, name = "INTERVAL", default_value = "100")]
    interval: u64,

    /// Number of active tables
    #[structopt(long, name = "TABLES", default_value = "10")]
    tables: u64,

    /// Number of rows logged per table per interval
    #[structopt(long, name = "ROWCOUNT")]
    rowcount: Option<Vec<u64>>,

    /// Number of columns logged per row
    #[structopt(long, name = "COLUMNS", default_value = "20")]
    columns: u64,

    /// Prefix for table names
    #[structopt(long, name = "PREFIX", default_value = "")]
    table_prefix: String,
}

#[tokio::main]
async fn main() {
    env_logger::init();
    let Opt {
        addr,
        interval,
        tables: n_tables,
        rowcount,
        columns,
        table_prefix,
    } = Opt::from_args();
    let rowcount = rowcount.unwrap_or_else(Vec::new);
    let tables: Vec<_> = (0..n_tables)
        .map(|i| format!("{table_prefix}{}_{i}", random_word::get(random_word::Lang::En),))
        .collect();
    let mut log = locustdb::logging_client::LoggingClient::new(
        Duration::from_secs(1),
        &addr,
        1 << 28,
        BufferFullPolicy::Block,
        None,
    );
    let mut interval = time::interval(Duration::from_millis(interval));

    loop {
        interval.tick().await;
        for (i, table) in tables.iter().enumerate() {
            for _ in 0..(rowcount.get(i).cloned().unwrap_or(1)) {
                log.log(
                    table,
                    (0..columns).map(|c| (format!("col_{c}"), vf64(rand::random::<f64>()))),
                );
            }
        }
    }
}
