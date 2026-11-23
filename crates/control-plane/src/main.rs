use anyhow::Context;
use axum::{Json, Router, extract::State, routing::get};
use clap::Parser;
use openmineros_common::status::HealthStatusResponse;
use openmineros_common::{
    BoardFamily, ChainStatus, ContributionStatus, FanState, FanStatus, FirmwareInfo, MinerStatus,
    Model, PoolStatus, SystemInfo,
};
use openmineros_supervisor::Supervisor;
use std::{net::SocketAddr, path::PathBuf, sync::Arc};
use tracing::info;

mod x19_api;

#[derive(Debug, Parser)]
#[command(name = "openmineros-control-plane")]
#[command(about = "OpenMinerOS local control plane")]
struct Cli {
    #[arg(long, default_value = "s19-xil")]
    board: String,
    #[arg(long, default_value = "s19j-pro")]
    model: String,
    #[arg(long, default_value = "127.0.0.1:8080")]
    listen: SocketAddr,
    /// Firmware identity written by the image build. Overrides --board.
    #[arg(long)]
    fw_info: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "openmineros_control_plane=info".into()),
        )
        .init();

    let cli = Cli::parse();
    let model: Model = cli.model.parse().context("invalid model")?;
    let firmware = match &cli.fw_info {
        Some(path) => FirmwareInfo::load(path)
            .with_context(|| format!("cannot load firmware info from {}", path.display()))?,
        None => {
            let board: BoardFamily = cli.board.parse().context("invalid board")?;
            FirmwareInfo::development(board)
        }
    };
    let supervisor = Arc::new(Supervisor::with_firmware(model, firmware)?);

    let app = Router::new()
        .route("/api/v1/system/info", get(system_info))
        .route("/api/v1/system/firmware", get(system_firmware))
        .route("/api/v1/system/health", get(system_health))
        .route("/api/v1/miner/status", get(miner_status))
        .route("/api/v1/chains", get(chains))
        .route("/api/v1/fans", get(fans))
        .route("/api/v1/pools", get(pools))
        .route("/api/v1/contribution/status", get(contribution_status))
        .route("/metrics", get(metrics))
        .with_state(supervisor)
        .merge(x19_api::routes())
        .layer(axum::middleware::from_fn(x19_api::guard));

    x19_api::spawn_sampler();
    let listener = tokio::net::TcpListener::bind(cli.listen).await?;
    info!(
        "OpenMinerOS control plane listening on http://{}",
        cli.listen
    );
    axum::serve(listener, app).await?;
    Ok(())
}

async fn system_info(State(supervisor): State<Arc<Supervisor>>) -> Json<SystemInfo> {
    Json(supervisor.system_info())
}

async fn system_firmware(State(supervisor): State<Arc<Supervisor>>) -> Json<FirmwareInfo> {
    Json(supervisor.firmware().clone())
}

async fn system_health(State(supervisor): State<Arc<Supervisor>>) -> Json<HealthStatusResponse> {
    Json(supervisor.health())
}

async fn miner_status(State(supervisor): State<Arc<Supervisor>>) -> Json<MinerStatus> {
    Json(supervisor.miner_status())
}

async fn chains(State(supervisor): State<Arc<Supervisor>>) -> Json<Vec<ChainStatus>> {
    Json(supervisor.chains())
}

async fn fans(State(supervisor): State<Arc<Supervisor>>) -> Json<Vec<FanStatus>> {
    Json(supervisor.fans())
}

async fn pools(State(supervisor): State<Arc<Supervisor>>) -> Json<Vec<PoolStatus>> {
    Json(supervisor.pools())
}

async fn contribution_status(
    State(supervisor): State<Arc<Supervisor>>,
) -> Json<ContributionStatus> {
    Json(supervisor.contribution_status())
}

async fn metrics(State(supervisor): State<Arc<Supervisor>>) -> String {
    let miner = supervisor.miner_status();
    let contribution = supervisor.contribution_status();
    let mut output = String::new();

    output.push_str(&format!("omo_miner_hashrate_ths {}\n", miner.hashrate_ths));
    output.push_str(&format!("omo_miner_power_watts {}\n", miner.power_w));
    output.push_str(&format!(
        "omo_miner_efficiency_j_th {}\n",
        miner.efficiency_j_th
    ));
    output.push_str(&format!(
        "omo_shares_accepted_total {}\n",
        miner.accepted_shares
    ));
    output.push_str(&format!(
        "omo_shares_rejected_total {}\n",
        miner.rejected_shares
    ));
    output.push_str(&format!("omo_hw_errors_total {}\n", miner.hw_errors));
    output.push_str(&format!(
        "omo_contribution_rate_percent {}\n",
        contribution.rate_percent
    ));

    for fan in supervisor.fans() {
        output.push_str(&format!(
            "omo_fan_up{{fan=\"{}\"}} {}\n",
            fan.id,
            u8::from(fan.state == FanState::Ok)
        ));
        output.push_str(&format!("omo_fan_rpm{{fan=\"{}\"}} {}\n", fan.id, fan.rpm));
    }

    for chain in supervisor.chains() {
        output.push_str(&format!(
            "omo_chain_up{{chain=\"{}\"}} {}\n",
            chain.id,
            u8::from(chain.present && chain.enabled)
        ));
        output.push_str(&format!(
            "omo_chain_asic_detected{{chain=\"{}\"}} {}\n",
            chain.id, chain.asic_detected
        ));
        output.push_str(&format!(
            "omo_chain_hashrate_ths{{chain=\"{}\"}} {}\n",
            chain.id, chain.hashrate_ths
        ));
        output.push_str(&format!(
            "omo_chain_hw_errors_total{{chain=\"{}\"}} {}\n",
            chain.id, chain.hw_errors
        ));
        output.push_str(&format!(
            "omo_temp_board_celsius{{chain=\"{}\"}} {}\n",
            chain.id, chain.temp_board_c
        ));
        output.push_str(&format!(
            "omo_temp_chip_max_celsius{{chain=\"{}\"}} {}\n",
            chain.id, chain.temp_chip_max_c
        ));
    }

    output
}
