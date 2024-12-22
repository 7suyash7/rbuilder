use std::path::PathBuf;

use clap::Parser;
use reth::revm::cached::CachedReads;
use reth_db::Database;
use reth_provider::{BlockReader, DatabaseProviderFactory, HeaderProvider, StateProviderFactory};
use serde::de::DeserializeOwned;
use std::fmt::Debug;
use sysperf::{format_results, gather_system_info, run_all_benchmarks};
use tokio::signal::ctrl_c;
use tokio_util::sync::CancellationToken;

use crate::{
    building::builders::{BacktestSimulateBlockInput, Block},
    live_builder::{
        base_config::load_config_toml_and_env, payload_events::MevBoostSlotDataGenerator,
    },
    telemetry,
    utils::build_info::Version,
};

use super::{
    base_config::{BaseConfig, MergeFromCli},
    LiveBuilder,
};

#[derive(Parser, Debug)]
enum Cli {
    #[clap(name = "run", about = "Run the builder")]
    Run(RunCmd),
    #[clap(name = "config", about = "Print the current config")]
    Config(RunCmd),
    #[clap(name = "version", about = "Print version information")]
    Version,
    #[clap(
        name = "sysperf",
        about = "Run system performance benchmarks (CPU, disk, memory)"
    )]
    SysPerf,
}

#[derive(Parser, Debug)]
struct RunCmd {
    #[clap(env = "RBUILDER_CONFIG", help = "Config file path")]
    config: PathBuf,

    #[command(flatten)]
    base: BaseCliArgs,

    #[command(flatten)]
    l1: L1CliArgs,
}

#[derive(Parser, Debug, Default)]
pub struct BaseCliArgs {
    #[arg(long, help = "Enable JSON logging format")]
    pub log_json: Option<bool>,

    #[arg(long, help = "Log level configuration string")]
    pub log_level: Option<String>,

    #[arg(long, help = "Port for full telemetry server")]
    pub full_telemetry_server_port: Option<u16>,

    #[arg(long, help = "IP address for full telemetry server")]
    pub full_telemetry_server_ip: Option<String>,

    #[arg(long, help = "Port for redacted telemetry server")]
    pub redacted_telemetry_server_port: Option<u16>,

    #[arg(long, help = "IP address for redacted telemetry server")]
    pub redacted_telemetry_server_ip: Option<String>,

    #[arg(long, help = "Enable colored log output")]
    pub log_color: Option<bool>,

    #[arg(long, help = "Enable dynamic logging to file")]
    pub log_enable_dynamic: Option<bool>,

    #[arg(long, help = "Path to store error logs")]
    pub error_storage_path: Option<PathBuf>,

    #[arg(long, help = "Coinbase signer secret key")]
    pub coinbase_secret_key: Option<String>,

    #[arg(long, help = "Flashbots database URL")]
    pub flashbots_db: Option<String>,

    #[arg(long, help = "JSON-RPC server port")]
    pub jsonrpc_server_port: Option<u16>,

    #[arg(long, help = "JSON-RPC server IP address")]
    pub jsonrpc_server_ip: Option<String>,

    #[arg(long, help = "Ignore cancellable orders")]
    pub ignore_cancellable_orders: Option<bool>,

    #[arg(long, help = "Ignore blob transactions")]
    pub ignore_blobs: Option<bool>,

    #[arg(long, help = "Chain identifier (mainnet/goerli/etc)")]
    pub chain: Option<String>,

    #[arg(long, help = "Path to reth data directory")]
    pub reth_datadir: Option<PathBuf>,
}

#[derive(Parser, Debug, Default)]
pub struct L1CliArgs {
    #[arg(long, help = "Enable dry run mode")]
    pub dry_run: Option<bool>,

    #[arg(long, help = "URLs for dry run validation")]
    pub dry_run_validation_url: Option<Vec<String>>,

    #[arg(long, help = "Enable optimistic submission mode")]
    pub optimistic_enabled: Option<bool>,

    #[arg(long, help = "Maximum bid value (ETH) for optimistic mode")]
    pub optimistic_max_bid_value_eth: Option<String>,

    #[arg(long, help = "Pre-validate optimistic blocks")]
    pub optimistic_prevalidate_optimistic_blocks: Option<bool>,

    #[arg(long, help = "Maximum concurrent block sealing operations")]
    pub max_concurrent_seals: Option<u64>,

    #[arg(long, help = "Consensus layer node URLs")]
    pub cl_node_url: Option<Vec<String>>,

    #[arg(long, help = "Genesis fork version override")]
    pub genesis_fork_version: Option<String>,
}

/// Basic stuff needed to call cli::run
pub trait LiveBuilderConfig: Debug + DeserializeOwned + Sync {
    fn base_config(&self) -> &BaseConfig;
    /// Version reported by telemetry
    fn version_for_telemetry(&self) -> Version;

    /// Create a concrete builder
    ///
    /// Desugared from async to future to keep clippy happy
    fn new_builder<P, DB>(
        &self,
        provider: P,
        cancellation_token: CancellationToken,
    ) -> impl std::future::Future<Output = eyre::Result<LiveBuilder<P, DB, MevBoostSlotDataGenerator>>>
           + Send
    where
        DB: Database + Clone + 'static,
        P: DatabaseProviderFactory<DB = DB, Provider: BlockReader>
            + StateProviderFactory
            + HeaderProvider
            + Clone
            + 'static;

    /// Patch until we have a unified way of backtesting using the exact algorithms we use on the LiveBuilder.
    /// building_algorithm_name will come from the specific configuration.
    fn build_backtest_block<P, DB>(
        &self,
        building_algorithm_name: &str,
        input: BacktestSimulateBlockInput<'_, P>,
    ) -> eyre::Result<(Block, CachedReads)>
    where
        DB: Database + Clone + 'static,
        P: DatabaseProviderFactory<DB = DB, Provider: BlockReader>
            + StateProviderFactory
            + Clone
            + 'static;
}

/// print_version_info func that will be called on command Cli::Version
/// on_run func that will be called on command Cli::Run just before running
pub async fn run<ConfigType>(print_version_info: fn(), on_run: Option<fn()>) -> eyre::Result<()>
where
    ConfigType: LiveBuilderConfig + MergeFromCli<BaseCliArgs> + MergeFromCli<L1CliArgs>,
{
    let cli = Cli::parse();
    let cli = match cli {
        Cli::Run(cli) => cli,
        Cli::Config(cli) => {
            let mut config: ConfigType = load_config_toml_and_env(cli.config)?;
            config.merge(&cli.base);
            config.merge(&cli.l1);
            println!("{:#?}", config);
            return Ok(());
        }
        Cli::Version => {
            print_version_info();
            return Ok(());
        }
        Cli::SysPerf => {
            let result =
                run_all_benchmarks(&PathBuf::from("/tmp/benchmark_test.tmp"), 100, 100, 1000)?;

            let sysinfo = gather_system_info();
            println!("{}", format_results(&result, &sysinfo));
            return Ok(());
        }
    };

    let mut config: ConfigType = load_config_toml_and_env(cli.config)?;
    config.merge(&cli.base);
    config.merge(&cli.l1);
    config.base_config().setup_tracing_subscriber()?;

    let cancel = CancellationToken::new();

    // Spawn redacted server that is safe for tdx builders to expose
    telemetry::servers::redacted::spawn(config.base_config().redacted_telemetry_server_address())
        .await?;

    // Spawn debug server that exposes detailed operational information
    telemetry::servers::full::spawn(
        config.base_config().full_telemetry_server_address(),
        config.version_for_telemetry(),
        config.base_config().log_enable_dynamic,
    )
    .await?;
    let provider = config.base_config().create_provider_factory()?;
    let builder = config.new_builder(provider, cancel.clone()).await?;

    let ctrlc = tokio::spawn(async move {
        ctrl_c().await.unwrap_or_default();
        cancel.cancel()
    });
    if let Some(on_run) = on_run {
        on_run();
    }
    builder.run().await?;

    ctrlc.await.unwrap_or_default();
    Ok(())
}
