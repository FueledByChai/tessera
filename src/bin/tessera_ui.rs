use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result, bail};
use axum::extract::{Path as AxumPath, Query, State};
use axum::http::{HeaderValue, Method, StatusCode};
use axum::response::Html;
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, Datelike, NaiveDate, NaiveTime, Utc};
use chrono_tz::America::Los_Angeles;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::process::Command;
use tokio::sync::Semaphore;
use tower_http::cors::CorsLayer;

use tessera::local_config::LocalConfig;
use tessera::portfolio::{
    CapitalMode, PortfolioComponentConfig, PortfolioConfig, RebalanceMethod, combine_portfolio,
};
use tessera::report::{ReportView, generate_report, load_report_view};
use tessera::sdk::manifest::Manifest as SdkManifest;
use tessera::sdk::runner::{
    Resolution as SdkResolution, SdkCostConfig, SdkDataConfig, SdkLimitsConfig, SdkRunConfig,
    SdkSizingConfig, SessionKind as SdkSessionKind,
};

const DEFAULT_START: &str = "2020-01-01";
const DEFAULT_END: &str = "2026-12-31";

#[derive(Clone)]
struct AppState {
    root: PathBuf,
    local: Arc<LocalConfig>,
    database: Arc<Mutex<Connection>>,
    workers: Arc<Semaphore>,
    instruments: Arc<Mutex<Option<InstrumentIndex>>>,
    sdk_manifests:
        Arc<Mutex<std::collections::HashMap<PathBuf, (std::time::SystemTime, Vec<SdkManifest>)>>>,
}

#[derive(Debug, Clone, Serialize)]
struct StrategyRecord {
    id: String,
    name: String,
    version: String,
    status: String,
    description: String,
    asset_scope: String,
    config_path: String,
    runnable: bool,
    base_strategy_id: Option<String>,
    source_sha256: Option<String>,
    custom: bool,
    /// Manifest id for SDK strategies (row ids may carry dev/release suffixes).
    sdk_strategy_id: Option<String>,
}

#[derive(Debug, Serialize)]
struct RunRecord {
    id: String,
    strategy_id: Option<String>,
    name: String,
    research_label: String,
    status: String,
    legacy: bool,
    report_path: Option<String>,
    artifact_dir: String,
    config_path: Option<String>,
    start_date: Option<String>,
    end_date: Option<String>,
    created_at: String,
    starred: bool,
    metrics_cached: bool,
    metrics: Option<RunMetrics>,
}

/// Headline performance figures cached per run so lists can sort without reopening reports.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct RunMetrics {
    cagr_percent: Option<f64>,
    total_return_percent: Option<f64>,
    sharpe: Option<f64>,
    sortino: Option<f64>,
    calmar: Option<f64>,
    max_drawdown_percent: Option<f64>,
    annual_volatility_percent: Option<f64>,
    win_rate_percent: Option<f64>,
    trades: Option<usize>,
    start: Option<String>,
    end: Option<String>,
}

#[derive(Debug, Deserialize)]
struct StarRequest {
    starred: bool,
}

#[derive(Debug, Serialize)]
struct JobRecord {
    id: String,
    run_id: String,
    strategy_id: String,
    status: String,
    start_date: String,
    end_date: String,
    created_at: String,
    started_at: Option<String>,
    finished_at: Option<String>,
    log_path: String,
    error: Option<String>,
    #[serde(skip_serializing)]
    parameters_json: String,
    costs_enabled: bool,
    /// Where a running job is, parsed from the engine's streamed `progress:` lines.
    #[serde(default, skip_deserializing)]
    progress: Option<JobProgress>,
}

#[derive(Debug, Clone, Serialize)]
struct JobProgress {
    stage: String,
    done: u64,
    total: u64,
    percent: f64,
    label: String,
    elapsed_seconds: u64,
}

/// Reads the tail of a running job's worker log and returns the latest progress line.
fn job_progress(root: &Path, job: &JobRecord) -> Option<JobProgress> {
    if job.status != "running" {
        return None;
    }
    let path = root.join(&job.log_path);
    let mut file = fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let tail = 16 * 1024;
    if len > tail {
        use std::io::Seek;
        file.seek(std::io::SeekFrom::Start(len - tail)).ok()?;
    }
    let mut text = String::new();
    use std::io::Read;
    file.read_to_string(&mut text).ok()?;
    let line = text
        .lines()
        .rev()
        .find(|line| line.starts_with("progress: "))?;
    parse_progress_line(line)
}

/// `progress: <stage> <done>/<total> <label...> elapsed=<seconds>s`
fn parse_progress_line(line: &str) -> Option<JobProgress> {
    let rest = line.strip_prefix("progress: ")?;
    let mut parts = rest.split_whitespace();
    let stage = parts.next()?.to_owned();
    let (done, total) = parts.next()?.split_once('/')?;
    let done: u64 = done.parse().ok()?;
    let total: u64 = total.parse().ok()?;
    let mut label = Vec::new();
    let mut elapsed_seconds = 0;
    for part in parts {
        if let Some(value) = part.strip_prefix("elapsed=") {
            elapsed_seconds = value.trim_end_matches('s').parse().unwrap_or(0);
        } else {
            label.push(part);
        }
    }
    let percent = if total == 0 {
        100.0
    } else {
        (done as f64 / total as f64 * 100.0).min(100.0)
    };
    Some(JobProgress {
        stage,
        done,
        total,
        percent,
        label: label.join(" "),
        elapsed_seconds,
    })
}

fn attach_progress(root: &Path, mut jobs: Vec<JobRecord>) -> Vec<JobRecord> {
    for job in &mut jobs {
        job.progress = job_progress(root, job);
    }
    jobs
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct CostProfileRecord {
    id: String,
    name: String,
    asset_class: String,
    model: String,
    entry_bps: f64,
    exit_bps: f64,
    tick_size: f64,
    entry_slippage_ticks: u32,
    exit_slippage_ticks: u32,
    entry_commission_per_unit: f64,
    exit_commission_per_unit: f64,
    minimum_commission: f64,
    created_at: String,
    builtin: bool,
}

#[derive(Debug, Deserialize)]
struct CreateCostProfileRequest {
    name: String,
    asset_class: String,
    model: String,
    #[serde(default)]
    entry_bps: f64,
    #[serde(default)]
    exit_bps: f64,
    #[serde(default = "default_tick_size")]
    tick_size: f64,
    #[serde(default)]
    entry_slippage_ticks: u32,
    #[serde(default)]
    exit_slippage_ticks: u32,
    #[serde(default)]
    entry_commission_per_unit: f64,
    #[serde(default)]
    exit_commission_per_unit: f64,
    #[serde(default)]
    minimum_commission: f64,
}

fn default_tick_size() -> f64 {
    0.01
}

#[derive(Debug, Clone, Serialize)]
struct AutomationScheduleRecord {
    id: String,
    name: String,
    kind: String,
    enabled: bool,
    local_time: String,
    weekdays: String,
    last_run_date: Option<String>,
    last_status: Option<String>,
    created_at: String,
}

#[derive(Debug, Clone, Serialize)]
struct StrategySourceFile {
    path: String,
    content: String,
    editable: bool,
}

#[derive(Debug, Clone, Serialize)]
struct StrategySourceResponse {
    strategy: StrategyRecord,
    files: Vec<StrategySourceFile>,
    source_sha256: String,
    immutable: bool,
}

#[derive(Debug, Clone, Serialize)]
struct StrategyDraftRecord {
    id: String,
    base_strategy_id: String,
    name: String,
    version: String,
    description: String,
    status: String,
    source_sha256: String,
    created_at: String,
    updated_at: String,
    last_validation_id: Option<String>,
    release_strategy_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct StrategyDraftDetail {
    draft: StrategyDraftRecord,
    files: Vec<StrategySourceFile>,
    validation: Option<StrategyValidationRecord>,
}

#[derive(Debug, Clone, Serialize)]
struct StrategyValidationRecord {
    id: String,
    draft_id: String,
    action: String,
    status: String,
    source_sha256: String,
    created_at: String,
    started_at: Option<String>,
    finished_at: Option<String>,
    log: String,
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CreateStrategyDraftRequest {
    base_strategy_id: String,
    #[serde(default)]
    strategy_id: Option<String>,
    name: String,
    version: String,
    description: String,
}

#[derive(Debug, Deserialize)]
struct SaveStrategyDraftFileRequest {
    path: String,
    content: String,
}

#[derive(Debug, Deserialize)]
struct ValidateStrategyDraftRequest {
    action: String,
}

#[derive(Debug, Deserialize)]
struct ReleaseStrategyDraftRequest {
    strategy_id: String,
    name: String,
    version: String,
    description: String,
}

#[derive(Debug, Deserialize)]
struct CreateAutomationScheduleRequest {
    name: String,
    kind: String,
    local_time: String,
    #[serde(default = "default_weekdays")]
    weekdays: String,
    #[serde(default)]
    enabled: bool,
}

fn default_weekdays() -> String {
    "mon,tue,wed,thu,fri".to_owned()
}

#[derive(Debug, Serialize)]
struct StrategyDetailResponse {
    instruments: Option<InstrumentRequirement>,
    sdk: Option<SdkManifest>,
    strategy: StrategyRecord,
    rules: Vec<String>,
    default_parameters: serde_json::Value,
    presets: Vec<PresetRecord>,
    runs: Vec<RunRecord>,
}

#[derive(Debug, Serialize)]
struct PresetRecord {
    id: String,
    strategy_id: String,
    name: String,
    parameters: serde_json::Value,
    costs_enabled: bool,
    created_at: String,
}

#[derive(Debug, Deserialize)]
struct SavePresetRequest {
    strategy_id: String,
    name: String,
    #[serde(default)]
    parameters: serde_json::Value,
    #[serde(default = "default_true")]
    costs_enabled: bool,
}

#[derive(Debug, Serialize)]
struct RunDetailResponse {
    run: RunRecord,
    report: Option<ReportView>,
    report_url: Option<String>,
    config_text: Option<String>,
    manifest: Option<serde_json::Value>,
    /// The worker's error for a failed job, so the run page can say why.
    job_error: Option<String>,
}

#[derive(Debug, Serialize)]
struct DashboardResponse {
    strategies: Vec<StrategyRecord>,
    recent_runs: Vec<RunRecord>,
    jobs: Vec<JobRecord>,
    production_strategies: usize,
    historical_reports: usize,
    active_jobs: usize,
    worker_capacity: usize,
}

#[derive(Debug, Serialize)]
struct HealthResponse {
    status: &'static str,
    database: &'static str,
    worker_capacity: usize,
}

#[derive(Debug, Serialize)]
struct ImportResponse {
    discovered: usize,
    imported: usize,
}

#[derive(Debug, Serialize)]
struct DataStatusResponse {
    latest_market_date: String,
    latest_spy_date: String,
    symbols_on_latest_date: usize,
    universe_symbols: usize,
    updated_at_utc: String,
    update_job: Option<DataUpdateRecord>,
}

#[derive(Debug, Serialize)]
struct DataUpdateRecord {
    id: String,
    status: String,
    created_at: String,
    started_at: Option<String>,
    finished_at: Option<String>,
    log_path: String,
    error: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct SweepAxis {
    parameter: String,
    values: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize)]
struct CreateSweepRequest {
    strategy_id: String,
    name: String,
    start_date: String,
    end_date: String,
    #[serde(default)]
    base_parameters: serde_json::Value,
    axes: Vec<SweepAxis>,
    #[serde(default = "default_true")]
    costs_enabled: bool,
    #[serde(default)]
    cost_profile_id: Option<String>,
}

#[derive(Debug, Serialize)]
struct SweepRecord {
    id: String,
    strategy_id: String,
    name: String,
    research_label: String,
    start_date: String,
    end_date: String,
    axes: Vec<SweepAxis>,
    costs_enabled: bool,
    created_at: String,
    status: String,
    configuration_count: usize,
    complete_count: usize,
    failed_count: usize,
}

#[derive(Debug, Serialize)]
struct SweepMetrics {
    sharpe: Option<f64>,
    cagr_percent: f64,
    max_drawdown_percent: f64,
    annual_volatility_percent: f64,
    trade_count: usize,
}

#[derive(Debug, Serialize)]
struct SweepMemberRecord {
    configuration_index: usize,
    run_id: String,
    job_id: String,
    status: String,
    parameters: serde_json::Value,
    metrics: Option<SweepMetrics>,
}

#[derive(Debug, Serialize)]
struct SweepDetailResponse {
    sweep: SweepRecord,
    members: Vec<SweepMemberRecord>,
}

#[derive(Debug, Deserialize)]
struct CreatePortfolioComponentRequest {
    run_id: String,
    weight: f64,
    #[serde(default)]
    capital_group: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CreatePortfolioRequest {
    name: String,
    #[serde(default = "default_initial_capital")]
    initial_capital: f64,
    capital_mode: String,
    components: Vec<CreatePortfolioComponentRequest>,
}

#[derive(Debug, Serialize)]
struct PortfolioRecord {
    id: String,
    run_id: String,
    name: String,
    capital_mode: String,
    initial_capital: f64,
    created_at: String,
    component_count: usize,
}

fn default_initial_capital() -> f64 {
    100_000.0
}

#[derive(Debug, Clone, Deserialize)]
struct CreateJobRequest {
    strategy_id: String,
    #[serde(default = "default_start")]
    start_date: String,
    #[serde(default = "default_end")]
    end_date: String,
    #[serde(default = "default_research_label")]
    research_label: String,
    name: Option<String>,
    #[serde(default)]
    parameters: serde_json::Value,
    #[serde(default = "default_true")]
    costs_enabled: bool,
    #[serde(default)]
    cost_profile_id: Option<String>,
}

fn default_true() -> bool {
    true
}

fn default_start() -> String {
    DEFAULT_START.to_owned()
}

fn default_end() -> String {
    DEFAULT_END.to_owned()
}

fn default_research_label() -> String {
    "Research".to_owned()
}

#[tokio::main]
async fn main() -> Result<()> {
    let root = std::env::var_os("TESSERA_ROOT")
        .map(PathBuf::from)
        .unwrap_or(std::env::current_dir().context("resolve working directory")?);
    let state_dir = root.join("data/ui");
    fs::create_dir_all(&state_dir).context("create UI state directory")?;
    let connection =
        Connection::open(state_dir.join("tessera_ui.sqlite3")).context("open UI catalog")?;
    migrate(&connection)?;
    recover_incomplete_jobs(&connection)?;
    seed_strategies(&connection)?;
    seed_cost_profiles(&connection)?;
    seed_automation_schedules(&connection)?;

    let local = LocalConfig::load(&root)?;
    eprintln!(
        "data library: {} ({})",
        local.data.daily_dir.display(),
        local.data.provider
    );
    let state = AppState {
        root,
        local: Arc::new(local),
        database: Arc::new(Mutex::new(connection)),
        workers: Arc::new(Semaphore::new(2)),
        instruments: Arc::new(Mutex::new(None)),
        sdk_manifests: Arc::new(Mutex::new(std::collections::HashMap::new())),
    };
    import_legacy_reports(&state)?;
    let metrics_state = state.clone();
    tokio::task::spawn_blocking(move || {
        if let Err(error) = backfill_run_metrics(&metrics_state, None, usize::MAX) {
            eprintln!("run metrics backfill failed: {error:#}");
        }
    });
    let sdk_state = state.clone();
    tokio::task::spawn_blocking(move || {
        if let Err(error) = sync_sdk_strategies(&sdk_state, None, None) {
            eprintln!("SDK strategy sync failed: {error:#}");
        }
    });
    let scheduler_state = state.clone();
    tokio::spawn(async move { automation_scheduler(scheduler_state).await });

    let cors = CorsLayer::new()
        .allow_origin([
            "http://127.0.0.1:3000".parse::<HeaderValue>()?,
            "http://localhost:3000".parse::<HeaderValue>()?,
            "http://127.0.0.1:3001".parse::<HeaderValue>()?,
            "http://localhost:3001".parse::<HeaderValue>()?,
            "http://127.0.0.1:3002".parse::<HeaderValue>()?,
            "http://localhost:3002".parse::<HeaderValue>()?,
            "http://127.0.0.1:3322".parse::<HeaderValue>()?,
            "http://localhost:3322".parse::<HeaderValue>()?,
        ])
        .allow_methods([Method::GET, Method::POST])
        .allow_headers([axum::http::header::CONTENT_TYPE]);

    let app = Router::new()
        .route("/api/health", get(health))
        .route("/api/dashboard", get(dashboard))
        .route("/api/strategies/{id}", get(strategy_detail))
        .route("/api/strategies/{id}/source", get(strategy_source))
        .route(
            "/api/strategy-drafts",
            get(list_strategy_drafts).post(create_strategy_draft),
        )
        .route("/api/strategy-drafts/{id}", get(strategy_draft_detail))
        .route(
            "/api/strategy-drafts/{id}/files",
            post(save_strategy_draft_file),
        )
        .route(
            "/api/strategy-drafts/{id}/validate",
            post(validate_strategy_draft),
        )
        .route(
            "/api/strategy-drafts/{id}/release",
            post(release_strategy_draft),
        )
        .route(
            "/api/strategy-validations/{id}",
            get(strategy_validation_detail),
        )
        .route("/api/presets", post(save_preset))
        .route("/api/legacy/import", post(import_legacy))
        .route("/api/jobs", get(list_jobs).post(create_job))
        .route("/api/runs", get(list_runs))
        .route("/api/runs/{id}", get(run_detail))
        .route("/api/runs/{id}/report", get(run_report))
        .route("/api/runs/{id}/star", post(set_run_star))
        .route("/api/sweeps", get(list_sweeps).post(create_sweep))
        .route("/api/sweeps/{id}", get(sweep_detail))
        .route(
            "/api/portfolios",
            get(list_portfolios).post(create_portfolio),
        )
        .route(
            "/api/cost-profiles",
            get(list_cost_profiles).post(create_cost_profile),
        )
        .route(
            "/api/automations",
            get(list_automations).post(create_automation),
        )
        .route("/api/automations/{id}/toggle", post(toggle_automation))
        .route("/api/automations/{id}/run", post(run_automation_now))
        .route("/api/data/status", get(data_status))
        .route("/api/instruments", get(search_instrument_catalog))
        .route(
            "/api/strategy-drafts/{id}/build",
            post(build_strategy_draft),
        )
        .route("/api/data/update-eod", post(start_eod_update))
        .layer(cors)
        .with_state(state);

    let address = "127.0.0.1:8787";
    let listener = tokio::net::TcpListener::bind(address).await?;
    println!("Tessera API listening on http://{address}");
    axum::serve(listener, app).await?;
    Ok(())
}

fn migrate(connection: &Connection) -> Result<()> {
    connection.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA foreign_keys = ON;
         CREATE TABLE IF NOT EXISTS strategies (
             id TEXT PRIMARY KEY,
             name TEXT NOT NULL,
             version TEXT NOT NULL,
             status TEXT NOT NULL,
             description TEXT NOT NULL,
             asset_scope TEXT NOT NULL,
             config_path TEXT NOT NULL,
             command_name TEXT,
             runnable INTEGER NOT NULL DEFAULT 0,
             created_at TEXT NOT NULL
         );
         CREATE TABLE IF NOT EXISTS runs (
             id TEXT PRIMARY KEY,
             strategy_id TEXT REFERENCES strategies(id),
             name TEXT NOT NULL,
             research_label TEXT NOT NULL,
             status TEXT NOT NULL,
             legacy INTEGER NOT NULL DEFAULT 0,
             artifact_dir TEXT NOT NULL UNIQUE,
             report_path TEXT,
             config_path TEXT,
             start_date TEXT,
             end_date TEXT,
             created_at TEXT NOT NULL,
             immutable INTEGER NOT NULL DEFAULT 1,
             exit_code INTEGER
         );
         CREATE TABLE IF NOT EXISTS jobs (
             id TEXT PRIMARY KEY,
             run_id TEXT NOT NULL UNIQUE REFERENCES runs(id),
             strategy_id TEXT NOT NULL REFERENCES strategies(id),
             status TEXT NOT NULL,
             start_date TEXT NOT NULL,
             end_date TEXT NOT NULL,
             created_at TEXT NOT NULL,
             started_at TEXT,
             finished_at TEXT,
             log_path TEXT NOT NULL,
             error TEXT,
             parameters_json TEXT NOT NULL DEFAULT '{}',
             costs_enabled INTEGER NOT NULL DEFAULT 1
         );
         CREATE TABLE IF NOT EXISTS strategy_presets (
             id TEXT PRIMARY KEY,
             strategy_id TEXT NOT NULL REFERENCES strategies(id),
             name TEXT NOT NULL,
             parameters_json TEXT NOT NULL,
             costs_enabled INTEGER NOT NULL DEFAULT 1,
             created_at TEXT NOT NULL,
             immutable INTEGER NOT NULL DEFAULT 1
         );
         CREATE TABLE IF NOT EXISTS data_updates (
             id TEXT PRIMARY KEY,
             status TEXT NOT NULL,
             created_at TEXT NOT NULL,
             started_at TEXT,
             finished_at TEXT,
             log_path TEXT NOT NULL,
             error TEXT
         );
         CREATE TABLE IF NOT EXISTS watchlist_runs (
             id TEXT PRIMARY KEY,
             strategy_id TEXT NOT NULL REFERENCES strategies(id),
             name TEXT NOT NULL,
             as_of_date TEXT NOT NULL,
             intended_trade_date TEXT NOT NULL,
             generated_at TEXT NOT NULL,
             artifact_dir TEXT NOT NULL UNIQUE,
             config_path TEXT NOT NULL,
             regime_ok INTEGER NOT NULL,
             candidate_count INTEGER NOT NULL,
             immutable INTEGER NOT NULL DEFAULT 1
         );
         CREATE TABLE IF NOT EXISTS sweeps (
             id TEXT PRIMARY KEY,
             strategy_id TEXT NOT NULL REFERENCES strategies(id),
             name TEXT NOT NULL,
             research_label TEXT NOT NULL,
             start_date TEXT NOT NULL,
             end_date TEXT NOT NULL,
             axes_json TEXT NOT NULL,
             costs_enabled INTEGER NOT NULL DEFAULT 1,
             created_at TEXT NOT NULL,
             immutable INTEGER NOT NULL DEFAULT 1
         );
         CREATE TABLE IF NOT EXISTS sweep_members (
             sweep_id TEXT NOT NULL REFERENCES sweeps(id),
             configuration_index INTEGER NOT NULL,
             run_id TEXT NOT NULL UNIQUE REFERENCES runs(id),
             job_id TEXT NOT NULL UNIQUE REFERENCES jobs(id),
             parameters_json TEXT NOT NULL,
             PRIMARY KEY (sweep_id, configuration_index)
         );
         CREATE TABLE IF NOT EXISTS portfolios (
             id TEXT PRIMARY KEY,
             run_id TEXT NOT NULL UNIQUE REFERENCES runs(id),
             name TEXT NOT NULL,
             capital_mode TEXT NOT NULL,
             initial_capital REAL NOT NULL,
             created_at TEXT NOT NULL,
             immutable INTEGER NOT NULL DEFAULT 1
         );
         CREATE TABLE IF NOT EXISTS portfolio_components (
             portfolio_id TEXT NOT NULL REFERENCES portfolios(id),
             component_index INTEGER NOT NULL,
             source_run_id TEXT NOT NULL REFERENCES runs(id),
             weight REAL NOT NULL,
             capital_group TEXT,
             PRIMARY KEY (portfolio_id, component_index),
             UNIQUE (portfolio_id, source_run_id)
         );
         CREATE TABLE IF NOT EXISTS cost_profiles (
             id TEXT PRIMARY KEY,
             name TEXT NOT NULL,
             asset_class TEXT NOT NULL,
             model TEXT NOT NULL,
             entry_bps REAL NOT NULL DEFAULT 0,
             exit_bps REAL NOT NULL DEFAULT 0,
             tick_size REAL NOT NULL DEFAULT 0.01,
             entry_slippage_ticks INTEGER NOT NULL DEFAULT 0,
             exit_slippage_ticks INTEGER NOT NULL DEFAULT 0,
             entry_commission_per_unit REAL NOT NULL DEFAULT 0,
             exit_commission_per_unit REAL NOT NULL DEFAULT 0,
             minimum_commission REAL NOT NULL DEFAULT 0,
             created_at TEXT NOT NULL,
             builtin INTEGER NOT NULL DEFAULT 0,
             immutable INTEGER NOT NULL DEFAULT 1
         );
         CREATE TABLE IF NOT EXISTS automation_schedules (
             id TEXT PRIMARY KEY,
             name TEXT NOT NULL,
             kind TEXT NOT NULL,
             enabled INTEGER NOT NULL DEFAULT 0,
             local_time TEXT NOT NULL,
             weekdays TEXT NOT NULL,
             last_run_date TEXT,
             last_status TEXT,
             created_at TEXT NOT NULL
         );
         CREATE TABLE IF NOT EXISTS strategy_drafts (
             id TEXT PRIMARY KEY,
             base_strategy_id TEXT NOT NULL REFERENCES strategies(id),
             name TEXT NOT NULL,
             version TEXT NOT NULL,
             description TEXT NOT NULL,
             status TEXT NOT NULL DEFAULT 'draft',
             source_paths_json TEXT NOT NULL,
             source_sha256 TEXT NOT NULL,
             created_at TEXT NOT NULL,
             updated_at TEXT NOT NULL,
             last_validation_id TEXT,
             release_strategy_id TEXT REFERENCES strategies(id)
         );
         CREATE TABLE IF NOT EXISTS strategy_validations (
             id TEXT PRIMARY KEY,
             draft_id TEXT NOT NULL REFERENCES strategy_drafts(id),
             action TEXT NOT NULL,
             status TEXT NOT NULL,
             source_sha256 TEXT NOT NULL,
             created_at TEXT NOT NULL,
             started_at TEXT,
             finished_at TEXT,
             log_path TEXT NOT NULL,
             error TEXT
         );
         CREATE INDEX IF NOT EXISTS idx_runs_created_at ON runs(created_at DESC);
         CREATE INDEX IF NOT EXISTS idx_jobs_status_created ON jobs(status, created_at DESC);
         CREATE INDEX IF NOT EXISTS idx_presets_strategy_created
         ON strategy_presets(strategy_id, created_at DESC);
         CREATE INDEX IF NOT EXISTS idx_data_updates_created
         ON data_updates(created_at DESC);
         CREATE INDEX IF NOT EXISTS idx_watchlist_runs_generated
         ON watchlist_runs(generated_at DESC);
         CREATE INDEX IF NOT EXISTS idx_sweeps_created
         ON sweeps(created_at DESC);
         CREATE INDEX IF NOT EXISTS idx_sweep_members_sweep
         ON sweep_members(sweep_id, configuration_index);
         CREATE INDEX IF NOT EXISTS idx_portfolios_created
         ON portfolios(created_at DESC);
         CREATE INDEX IF NOT EXISTS idx_portfolio_components_portfolio
         ON portfolio_components(portfolio_id, component_index);
         CREATE INDEX IF NOT EXISTS idx_cost_profiles_created
         ON cost_profiles(created_at DESC);
         CREATE INDEX IF NOT EXISTS idx_automation_enabled_kind
         ON automation_schedules(enabled, kind);
         CREATE INDEX IF NOT EXISTS idx_strategy_drafts_updated
         ON strategy_drafts(updated_at DESC);
         CREATE INDEX IF NOT EXISTS idx_strategy_validations_draft_created
         ON strategy_validations(draft_id, created_at DESC);",
    )?;
    ensure_column(
        connection,
        "jobs",
        "parameters_json",
        "TEXT NOT NULL DEFAULT '{}'",
    )?;
    ensure_column(connection, "strategies", "base_strategy_id", "TEXT")?;
    ensure_column(connection, "strategies", "source_paths_json", "TEXT")?;
    ensure_column(connection, "strategies", "source_sha256", "TEXT")?;
    ensure_column(connection, "strategies", "source_bundle_path", "TEXT")?;
    ensure_column(connection, "strategies", "engine_path", "TEXT")?;
    ensure_column(connection, "strategies", "released_at", "TEXT")?;
    ensure_column(connection, "strategies", "sdk_strategy_id", "TEXT")?;
    // Hidden base row so SDK drafts satisfy the drafts table's foreign key.
    connection.execute(
        "INSERT OR IGNORE INTO strategies
         (id, name, version, status, description, asset_scope, config_path, command_name, runnable, created_at)
         VALUES ('sdk', 'Strategy SDK', 'template', 'Template',
                 'Base identity for one-file SDK strategy drafts', 'Any', 'sdk', 'run-strategy', 0, ?1)",
        [Utc::now().to_rfc3339()],
    )?;
    ensure_column(
        connection,
        "jobs",
        "costs_enabled",
        "INTEGER NOT NULL DEFAULT 1",
    )?;
    ensure_column(connection, "jobs", "cost_profile_id", "TEXT")?;
    ensure_column(connection, "runs", "starred", "INTEGER NOT NULL DEFAULT 0")?;
    ensure_column(connection, "runs", "metrics_json", "TEXT")?;
    ensure_column(
        connection,
        "jobs",
        "cost_profile_snapshot_json",
        "TEXT NOT NULL DEFAULT '{}'",
    )?;
    connection.execute_batch("PRAGMA optimize;")?;
    Ok(())
}

fn ensure_column(
    connection: &Connection,
    table: &str,
    column: &str,
    definition: &str,
) -> Result<()> {
    let mut statement = connection.prepare(&format!("PRAGMA table_info({table})"))?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if !columns.iter().any(|existing| existing == column) {
        connection.execute_batch(&format!(
            "ALTER TABLE {table} ADD COLUMN {column} {definition}"
        ))?;
    }
    Ok(())
}

fn seed_strategies(connection: &Connection) -> Result<()> {
    // Strategies come from compiled SDK manifests (see sync_sdk_strategies). Rows left behind
    // by earlier builds stay visible for their run history but can no longer be queued.
    connection.execute(
        "UPDATE strategies SET runnable = 0
         WHERE id != 'sdk' AND (base_strategy_id IS NULL OR base_strategy_id != 'sdk')",
        [],
    )?;
    Ok(())
}

fn seed_cost_profiles(connection: &Connection) -> Result<()> {
    let now = Utc::now().to_rfc3339();
    let profiles = [
        (
            "us-equities-default",
            "US equities · 1 tick + $0.005/share",
            "US equities",
            "fixed_tick_per_unit",
            0.0,
            0.0,
            0.01,
            1,
            1,
            0.005,
            0.005,
            0.0,
        ),
        (
            "us-equities-bps10",
            "US equities · 10 bps round trip",
            "US equities",
            "all_in_bps",
            5.0,
            5.0,
            0.01,
            0,
            0,
            0.0,
            0.0,
            0.0,
        ),
        (
            "fx-bps4",
            "Spot FX · 4 bps round trip",
            "Spot FX",
            "all_in_bps",
            2.0,
            2.0,
            0.0001,
            0,
            0,
            0.0,
            0.0,
            0.0,
        ),
        (
            "crypto-spot-bps10",
            "Crypto spot · 10 bps round trip",
            "Crypto spot",
            "all_in_bps",
            5.0,
            5.0,
            0.01,
            0,
            0,
            0.0,
            0.0,
            0.0,
        ),
        (
            "mnq-futures-research",
            "MNQ futures · 1 tick/fill + $2.50 round turn",
            "US futures",
            "fixed_tick_per_unit",
            0.0,
            0.0,
            0.25,
            1,
            1,
            1.25,
            1.25,
            0.0,
        ),
        (
            "costs-off",
            "Costs off · gross alpha",
            "Any",
            "none",
            0.0,
            0.0,
            0.01,
            0,
            0,
            0.0,
            0.0,
            0.0,
        ),
    ];
    for profile in profiles {
        connection.execute(
            "INSERT OR IGNORE INTO cost_profiles
             (id, name, asset_class, model, entry_bps, exit_bps, tick_size,
              entry_slippage_ticks, exit_slippage_ticks, entry_commission_per_unit,
              exit_commission_per_unit, minimum_commission, created_at, builtin, immutable)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, 1, 1)",
            params![
                profile.0, profile.1, profile.2, profile.3, profile.4, profile.5, profile.6,
                profile.7, profile.8, profile.9, profile.10, profile.11, now
            ],
        )?;
    }
    Ok(())
}

fn seed_automation_schedules(connection: &Connection) -> Result<()> {
    let now = Utc::now().to_rfc3339();
    connection.execute(
        "INSERT OR IGNORE INTO automation_schedules
         (id, name, kind, enabled, local_time, weekdays, created_at)
         VALUES ('weekday-eod-refresh', 'Weekday US EOD refresh', 'data_update', 0,
                 '19:15', 'mon,tue,wed,thu,fri', ?1)",
        [&now],
    )?;
    connection.execute(
        "INSERT OR IGNORE INTO automation_schedules
         (id, name, kind, enabled, local_time, weekdays, created_at)
         VALUES ('weekday-overnight-watchlist', 'Weekday Overnight Attention watchlist',
                 'watchlist', 0, '20:15', 'mon,tue,wed,thu,fri', ?1)",
        [&now],
    )?;
    Ok(())
}

fn validate_cost_profile_request(request: &CreateCostProfileRequest) -> Result<()> {
    anyhow::ensure!(
        !request.name.trim().is_empty() && request.name.trim().len() <= 100,
        "cost profile name must contain 1 to 100 characters"
    );
    anyhow::ensure!(
        ["US equities", "US futures", "Spot FX", "Crypto spot", "Any"]
            .contains(&request.asset_class.as_str()),
        "unsupported asset class"
    );
    anyhow::ensure!(
        ["fixed_tick_per_unit", "all_in_bps", "none"].contains(&request.model.as_str()),
        "unsupported cost model"
    );
    for value in [request.entry_bps, request.exit_bps] {
        anyhow::ensure!(
            value.is_finite() && (0.0..=500.0).contains(&value),
            "basis-point costs must be between 0 and 500 bps per side"
        );
    }
    anyhow::ensure!(
        request.tick_size.is_finite() && request.tick_size > 0.0,
        "tick size must be positive"
    );
    anyhow::ensure!(
        request.entry_slippage_ticks <= 100 && request.exit_slippage_ticks <= 100,
        "slippage cannot exceed 100 ticks per side"
    );
    for value in [
        request.entry_commission_per_unit,
        request.exit_commission_per_unit,
        request.minimum_commission,
    ] {
        anyhow::ensure!(
            value.is_finite() && (0.0..=100.0).contains(&value),
            "commission values must be between 0 and 100"
        );
    }
    Ok(())
}

fn map_cost_profile(row: &rusqlite::Row<'_>) -> rusqlite::Result<CostProfileRecord> {
    Ok(CostProfileRecord {
        id: row.get(0)?,
        name: row.get(1)?,
        asset_class: row.get(2)?,
        model: row.get(3)?,
        entry_bps: row.get(4)?,
        exit_bps: row.get(5)?,
        tick_size: row.get(6)?,
        entry_slippage_ticks: row.get::<_, i64>(7)? as u32,
        exit_slippage_ticks: row.get::<_, i64>(8)? as u32,
        entry_commission_per_unit: row.get(9)?,
        exit_commission_per_unit: row.get(10)?,
        minimum_commission: row.get(11)?,
        created_at: row.get(12)?,
        builtin: row.get::<_, i64>(13)? != 0,
    })
}

fn load_cost_profiles(state: &AppState) -> Result<Vec<CostProfileRecord>> {
    let connection = state.database.lock().expect("database lock poisoned");
    let mut statement = connection.prepare(
        "SELECT id, name, asset_class, model, entry_bps, exit_bps, tick_size,
                entry_slippage_ticks, exit_slippage_ticks, entry_commission_per_unit,
                exit_commission_per_unit, minimum_commission, created_at, builtin
         FROM cost_profiles ORDER BY builtin DESC, created_at DESC",
    )?;
    let rows = statement.query_map([], map_cost_profile)?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

fn load_cost_profile(state: &AppState, id: &str) -> Result<CostProfileRecord> {
    let connection = state.database.lock().expect("database lock poisoned");
    Ok(connection.query_row(
        "SELECT id, name, asset_class, model, entry_bps, exit_bps, tick_size,
                entry_slippage_ticks, exit_slippage_ticks, entry_commission_per_unit,
                exit_commission_per_unit, minimum_commission, created_at, builtin
         FROM cost_profiles WHERE id=?1",
        [id],
        map_cost_profile,
    )?)
}

fn validate_profile_compatibility(strategy_id: &str, _profile: &CostProfileRecord) -> Result<()> {
    anyhow::ensure!(
        strategy_id == "sdk",
        "this strategy is not runnable in this build; only SDK strategies can run"
    );
    Ok(())
}

fn validate_automation_request(request: &CreateAutomationScheduleRequest) -> Result<()> {
    anyhow::ensure!(
        !request.name.trim().is_empty() && request.name.trim().len() <= 100,
        "automation name must contain 1 to 100 characters"
    );
    anyhow::ensure!(
        ["data_update", "watchlist"].contains(&request.kind.as_str()),
        "unsupported automation kind"
    );
    NaiveTime::parse_from_str(&request.local_time, "%H:%M").context("local_time must use HH:MM")?;
    let allowed = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"];
    let days = request.weekdays.split(',').collect::<Vec<_>>();
    anyhow::ensure!(
        !days.is_empty() && days.iter().all(|day| allowed.contains(day)),
        "weekdays must be a comma-separated list of mon through sun"
    );
    Ok(())
}

fn map_automation(row: &rusqlite::Row<'_>) -> rusqlite::Result<AutomationScheduleRecord> {
    Ok(AutomationScheduleRecord {
        id: row.get(0)?,
        name: row.get(1)?,
        kind: row.get(2)?,
        enabled: row.get::<_, i64>(3)? != 0,
        local_time: row.get(4)?,
        weekdays: row.get(5)?,
        last_run_date: row.get(6)?,
        last_status: row.get(7)?,
        created_at: row.get(8)?,
    })
}

fn load_automations(state: &AppState) -> Result<Vec<AutomationScheduleRecord>> {
    let connection = state.database.lock().expect("database lock poisoned");
    let mut statement = connection.prepare(
        "SELECT id, name, kind, enabled, local_time, weekdays, last_run_date, last_status, created_at
         FROM automation_schedules ORDER BY created_at",
    )?;
    let rows = statement.query_map([], map_automation)?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

fn load_automation(state: &AppState, id: &str) -> Result<AutomationScheduleRecord> {
    let connection = state.database.lock().expect("database lock poisoned");
    Ok(connection.query_row(
        "SELECT id, name, kind, enabled, local_time, weekdays, last_run_date, last_status, created_at
         FROM automation_schedules WHERE id=?1",
        [id],
        map_automation,
    )?)
}

async fn automation_scheduler(state: AppState) {
    loop {
        if let Ok(due) = due_automation_ids(&state) {
            for id in due {
                let _ = execute_automation(&state, &id).await;
            }
        }
        tokio::time::sleep(std::time::Duration::from_secs(30)).await;
    }
}

fn due_automation_ids(state: &AppState) -> Result<Vec<String>> {
    let local = Utc::now().with_timezone(&Los_Angeles);
    let day = match local.weekday() {
        chrono::Weekday::Mon => "mon",
        chrono::Weekday::Tue => "tue",
        chrono::Weekday::Wed => "wed",
        chrono::Weekday::Thu => "thu",
        chrono::Weekday::Fri => "fri",
        chrono::Weekday::Sat => "sat",
        chrono::Weekday::Sun => "sun",
    };
    let now_time = local.format("%H:%M").to_string();
    let today = local.date_naive().to_string();
    Ok(load_automations(state)?
        .into_iter()
        .filter(|item| item.enabled)
        .filter(|item| item.weekdays.split(',').any(|value| value == day))
        .filter(|item| item.local_time <= now_time)
        .filter(|item| item.last_run_date.as_deref() != Some(today.as_str()))
        .map(|item| item.id)
        .collect())
}

async fn execute_automation(state: &AppState, id: &str) -> Result<()> {
    let schedule = load_automation(state, id)?;
    let local_date = Utc::now()
        .with_timezone(&Los_Angeles)
        .date_naive()
        .to_string();
    {
        let connection = state.database.lock().expect("database lock poisoned");
        connection.execute(
            "UPDATE automation_schedules SET last_run_date=?2, last_status='running' WHERE id=?1",
            params![id, local_date],
        )?;
    }
    let outcome = match schedule.kind.as_str() {
        "data_update" => queue_eod_update(state).map(|record| format!("queued {}", record.id)),
        _ => bail!("unsupported automation kind"),
    };
    let status = outcome.unwrap_or_else(|error| format!("failed: {error:#}"));
    let connection = state.database.lock().expect("database lock poisoned");
    connection.execute(
        "UPDATE automation_schedules SET last_status=?2 WHERE id=?1",
        params![id, status],
    )?;
    Ok(())
}

fn recover_incomplete_jobs(connection: &Connection) -> Result<()> {
    let finished_at = Utc::now().to_rfc3339();
    connection.execute(
        "UPDATE jobs SET status='failed', finished_at=?1,
         error='Local service restarted before this worker finished; output was preserved for inspection.'
         WHERE status IN ('queued', 'running')",
        [&finished_at],
    )?;
    connection.execute(
        "UPDATE runs SET status='Interrupted'
         WHERE id IN (SELECT run_id FROM jobs WHERE status='failed' AND finished_at=?1)",
        [&finished_at],
    )?;
    connection.execute(
        "UPDATE data_updates SET status='failed', finished_at=?1,
         error='Local service restarted before this data update finished; inspect the preserved log.'
         WHERE status IN ('queued', 'running')",
        [&finished_at],
    )?;
    connection.execute(
        "UPDATE strategy_validations SET status='failed', finished_at=?1,
         error='Local service restarted before validation finished; the draft source was preserved.'
         WHERE status IN ('queued', 'running')",
        [&finished_at],
    )?;
    Ok(())
}

async fn health(State(state): State<AppState>) -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ready",
        database: "ready",
        worker_capacity: state.workers.available_permits() + active_worker_count(&state),
    })
}

async fn dashboard(State(state): State<AppState>) -> Result<Json<DashboardResponse>, ApiError> {
    Ok(Json(load_dashboard(&state)?))
}

fn source_paths_for_family(strategy_id: &str) -> Result<Vec<&'static str>> {
    bail!(
        "strategy {strategy_id} does not expose a source bundle; SDK strategies record their file on the catalog row"
    )
}

// ---------------------------------------------------------------------------
// Instrument search (BT-201): catalog + on-disk coverage index for the picker.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct InstrumentDataRoots {
    daily_dir: PathBuf,
    five_minute_dir: PathBuf,
    one_minute_dir: PathBuf,
    universe_dir: PathBuf,
}

#[derive(Debug, Clone, Serialize)]
struct InstrumentRecord {
    symbol: String,
    code: String,
    suffix: String,
    name: String,
    exchange: String,
    asset_class: String,
    currency: String,
    status: String,
    daily: bool,
    five_minute: bool,
    one_minute: bool,
    #[serde(skip)]
    name_upper: String,
}

#[derive(Debug)]
struct InstrumentIndex {
    built_at: std::time::Instant,
    indexed_at: String,
    roots: InstrumentDataRoots,
    records: Vec<InstrumentRecord>,
}

#[derive(Debug, Default, Deserialize)]
struct InstrumentQuery {
    q: Option<String>,
    symbols: Option<String>,
    asset_class: Option<String>,
    suffix: Option<String>,
    resolution: Option<String>,
    limit: Option<usize>,
    refresh: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
struct CoverageRange {
    first: String,
    last: String,
}

#[derive(Debug, Clone, Serialize)]
struct InstrumentHit {
    #[serde(flatten)]
    record: InstrumentRecord,
    coverage: std::collections::BTreeMap<String, CoverageRange>,
    missing_resolutions: Vec<String>,
}

#[derive(Debug, Serialize)]
struct InstrumentSearchResponse {
    instruments: Vec<InstrumentHit>,
    total_matches: usize,
    index_size: usize,
    indexed_at: String,
}

/// Describes which instrument parameter a strategy family exposes and what data it needs.
#[derive(Debug, Clone, Serialize)]
struct InstrumentRequirement {
    parameter: &'static str,
    mode: &'static str,
    resolutions: Vec<&'static str>,
    suffixes: Vec<&'static str>,
    asset_classes: Vec<&'static str>,
    maximum: usize,
    note: &'static str,
}

fn instrument_requirement(family: &str) -> Option<InstrumentRequirement> {
    match family {
        "sdk" => Some(InstrumentRequirement {
            parameter: "symbols",
            mode: "multiple",
            resolutions: vec![],
            suffixes: vec![],
            asset_classes: vec![],
            maximum: 50,
            note: "One strategy instance per symbol on a shared account. Every symbol needs bars at the selected resolution.",
        }),
        _ => None,
    }
}

fn instrument_data_roots(state: &AppState) -> Result<InstrumentDataRoots> {
    let data = &state.local.data;
    Ok(InstrumentDataRoots {
        daily_dir: data.daily_dir.clone(),
        five_minute_dir: data.five_minute_dir.clone(),
        one_minute_dir: data.one_minute_dir.clone(),
        universe_dir: data.catalog_dir.clone(),
    })
}

fn list_csv_stems(dir: &Path) -> std::collections::HashSet<String> {
    let mut stems = std::collections::HashSet::new();
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            if let Some(stem) = name.strip_suffix(".csv") {
                stems.insert(stem.to_owned());
            }
        }
    }
    stems
}

fn classify_instrument(kind: &str, suffix: &str) -> String {
    match (suffix, kind) {
        ("CC", _) => "Crypto".to_owned(),
        ("FOREX", _) => "FX".to_owned(),
        ("GBOND", _) => "Government bond".to_owned(),
        (_, "Common Stock") => "Common Stock".to_owned(),
        (_, "ETF") | (_, "ETC") => "ETF".to_owned(),
        (_, "FUND") | (_, "Mutual Fund") => "Fund".to_owned(),
        (_, "Preferred Stock") => "Preferred".to_owned(),
        (_, "INDEX") => "Index".to_owned(),
        (_, "") => "Unknown".to_owned(),
        (_, other) => other.to_owned(),
    }
}

fn read_instrument_catalog(
    path: &Path,
    suffix: &str,
    status: &str,
    records: &mut Vec<InstrumentRecord>,
    seen: &mut std::collections::HashSet<String>,
) -> Result<()> {
    if !path.is_file() {
        return Ok(());
    }
    let mut reader = csv::ReaderBuilder::new()
        .flexible(true)
        .from_path(path)
        .with_context(|| format!("failed to open catalog {}", path.display()))?;
    let headers = reader.headers()?.clone();
    let column = |name: &str| headers.iter().position(|header| header == name);
    let code_column =
        column("Code").with_context(|| format!("catalog {} has no Code column", path.display()))?;
    let (name_column, exchange_column, currency_column, type_column) = (
        column("Name"),
        column("Exchange"),
        column("Currency"),
        column("Type"),
    );
    for row in reader.records() {
        let row = row?;
        let code = row.get(code_column).unwrap_or("").trim();
        if code.is_empty() {
            continue;
        }
        let symbol = format!("{code}.{suffix}");
        if !seen.insert(symbol.clone()) {
            continue;
        }
        let field = |index: Option<usize>| {
            index
                .and_then(|index| row.get(index))
                .unwrap_or("")
                .trim()
                .to_owned()
        };
        let exchange = field(exchange_column);
        let name = field(name_column);
        records.push(InstrumentRecord {
            symbol,
            code: code.to_owned(),
            suffix: suffix.to_owned(),
            name_upper: name.to_ascii_uppercase(),
            name,
            exchange: if exchange.is_empty() {
                suffix.to_owned()
            } else {
                exchange
            },
            asset_class: classify_instrument(&field(type_column), suffix),
            currency: field(currency_column),
            status: status.to_owned(),
            daily: false,
            five_minute: false,
            one_minute: false,
        });
    }
    Ok(())
}

fn build_instrument_index(roots: InstrumentDataRoots) -> Result<InstrumentIndex> {
    let mut records = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for (relative, suffix, status) in [
        ("catalog.csv", "US", "active"),
        ("delisted/catalog.csv", "US", "delisted"),
        ("CC/catalog.csv", "CC", "active"),
        ("FOREX/catalog.csv", "FOREX", "active"),
    ] {
        read_instrument_catalog(
            &roots.universe_dir.join(relative),
            suffix,
            status,
            &mut records,
            &mut seen,
        )?;
    }
    let daily = list_csv_stems(&roots.daily_dir);
    let five_minute = list_csv_stems(&roots.five_minute_dir);
    let one_minute = list_csv_stems(&roots.one_minute_dir);
    for record in &mut records {
        record.daily = daily.contains(&record.symbol);
        record.five_minute = five_minute.contains(&record.symbol);
        record.one_minute = one_minute.contains(&record.symbol);
    }
    // Data files that no catalog describes stay searchable by symbol alone.
    let mut uncataloged: Vec<String> = daily
        .iter()
        .chain(five_minute.iter())
        .chain(one_minute.iter())
        .filter(|symbol| !seen.contains(*symbol))
        .cloned()
        .collect();
    uncataloged.sort();
    uncataloged.dedup();
    for symbol in uncataloged {
        let (code, suffix) = symbol
            .rsplit_once('.')
            .map(|(code, suffix)| (code.to_owned(), suffix.to_owned()))
            .unwrap_or_else(|| (symbol.clone(), String::new()));
        records.push(InstrumentRecord {
            daily: daily.contains(&symbol),
            five_minute: five_minute.contains(&symbol),
            one_minute: one_minute.contains(&symbol),
            asset_class: classify_instrument("", &suffix),
            exchange: suffix.clone(),
            symbol,
            code,
            suffix,
            name: String::new(),
            name_upper: String::new(),
            currency: String::new(),
            status: "uncataloged".to_owned(),
        });
    }
    records.sort_by(|a, b| a.symbol.cmp(&b.symbol));
    Ok(InstrumentIndex {
        built_at: std::time::Instant::now(),
        indexed_at: Utc::now().to_rfc3339(),
        roots,
        records,
    })
}

fn comma_list(value: Option<&str>) -> Vec<String> {
    value
        .unwrap_or("")
        .split(',')
        .map(|item| item.trim().to_owned())
        .filter(|item| !item.is_empty())
        .collect()
}

fn has_resolution(record: &InstrumentRecord, resolution: &str) -> bool {
    match resolution.to_ascii_lowercase().as_str() {
        "daily" | "eod" | "d" | "1d" => record.daily,
        "5m" | "five_minute" => record.five_minute,
        "1m" | "one_minute" => record.one_minute,
        _ => true,
    }
}

fn search_instruments<'a>(
    index: &'a InstrumentIndex,
    query: &InstrumentQuery,
) -> (Vec<&'a InstrumentRecord>, usize) {
    let needle = query.q.as_deref().unwrap_or("").trim().to_ascii_uppercase();
    let exact: Option<std::collections::HashSet<String>> = query.symbols.as_deref().map(|list| {
        comma_list(Some(list))
            .into_iter()
            .map(|item| item.to_ascii_uppercase())
            .collect()
    });
    let suffixes: Vec<String> = comma_list(query.suffix.as_deref())
        .into_iter()
        .map(|item| item.to_ascii_uppercase())
        .collect();
    let classes = comma_list(query.asset_class.as_deref());
    // Resolution filters narrow open searches; exact symbol lookups report gaps instead.
    let resolutions = if exact.is_some() {
        Vec::new()
    } else {
        comma_list(query.resolution.as_deref())
    };
    let limit = query.limit.unwrap_or(25).clamp(1, 200);
    let mut scored: Vec<(u8, &InstrumentRecord)> = Vec::new();
    for record in &index.records {
        if let Some(exact) = &exact {
            let matches = exact.contains(&record.symbol)
                || (record.suffix == "US" && exact.contains(&record.code));
            if !matches {
                continue;
            }
        }
        if !suffixes.is_empty() && !suffixes.contains(&record.suffix) {
            continue;
        }
        if !classes.is_empty()
            && !classes
                .iter()
                .any(|class| class.eq_ignore_ascii_case(&record.asset_class))
        {
            continue;
        }
        if !resolutions
            .iter()
            .all(|resolution| has_resolution(record, resolution))
        {
            continue;
        }
        let rank = if exact.is_some() || needle.is_empty() {
            5
        } else if record.code == needle || record.symbol == needle {
            0
        } else if record.code.starts_with(&needle) {
            1
        } else if record.name_upper.starts_with(&needle) {
            2
        } else if record
            .name_upper
            .split_whitespace()
            .any(|word| word.starts_with(&needle))
        {
            3
        } else if record.name_upper.contains(&needle) || record.symbol.contains(&needle) {
            4
        } else {
            continue;
        };
        scored.push((rank, record));
    }
    let total = scored.len();
    scored.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then_with(|| b.1.daily.cmp(&a.1.daily))
            .then_with(|| (a.1.status != "active").cmp(&(b.1.status != "active")))
            .then_with(|| a.1.code.len().cmp(&b.1.code.len()))
            .then_with(|| a.1.symbol.cmp(&b.1.symbol))
    });
    (
        scored
            .into_iter()
            .take(limit)
            .map(|(_, record)| record)
            .collect(),
        total,
    )
}

fn extract_bar_date(line: &str, intraday: bool) -> Option<String> {
    if intraday {
        let start = line.find('"')? + 1;
        line.get(start..start + 10).map(str::to_owned)
    } else {
        line.split(',')
            .next()
            .filter(|date| date.len() == 10)
            .map(str::to_owned)
    }
}

/// Reads only the first data row and the tail of a bar file to report its date span.
fn csv_date_range(path: &Path, intraday: bool) -> Option<CoverageRange> {
    use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
    let mut file = fs::File::open(path).ok()?;
    let first_line = {
        let mut reader = BufReader::new(&file);
        let mut header = String::new();
        reader.read_line(&mut header).ok()?;
        let mut first = String::new();
        reader.read_line(&mut first).ok()?;
        first
    };
    let length = file.metadata().ok()?.len();
    let tail_length = length.min(4096);
    file.seek(SeekFrom::Start(length - tail_length)).ok()?;
    let mut tail = Vec::with_capacity(tail_length as usize);
    file.read_to_end(&mut tail).ok()?;
    let tail = String::from_utf8_lossy(&tail);
    let last_line = tail.lines().rev().find(|line| !line.trim().is_empty())?;
    Some(CoverageRange {
        first: extract_bar_date(first_line.trim(), intraday)?,
        last: extract_bar_date(last_line.trim(), intraday)?,
    })
}

fn instrument_search_blocking(
    state: &AppState,
    query: &InstrumentQuery,
) -> Result<InstrumentSearchResponse> {
    let mut guard = state
        .instruments
        .lock()
        .expect("instrument index lock poisoned");
    let stale = query.refresh.unwrap_or(false)
        || guard.as_ref().map_or(true, |index| {
            index.built_at.elapsed() > std::time::Duration::from_secs(600)
        });
    if stale {
        *guard = Some(build_instrument_index(instrument_data_roots(state)?)?);
    }
    let index = guard.as_ref().expect("instrument index is built");
    let (hits, total_matches) = search_instruments(index, query);
    let required = comma_list(query.resolution.as_deref());
    let instruments = hits
        .into_iter()
        .map(|record| {
            let mut coverage = std::collections::BTreeMap::new();
            let file_name = format!("{}.csv", record.symbol);
            for (label, present, dir, intraday) in [
                ("daily", record.daily, &index.roots.daily_dir, false),
                ("5m", record.five_minute, &index.roots.five_minute_dir, true),
                ("1m", record.one_minute, &index.roots.one_minute_dir, true),
            ] {
                if present {
                    if let Some(range) = csv_date_range(&dir.join(&file_name), intraday) {
                        coverage.insert(label.to_owned(), range);
                    }
                }
            }
            InstrumentHit {
                missing_resolutions: required
                    .iter()
                    .filter(|resolution| !has_resolution(record, resolution))
                    .cloned()
                    .collect(),
                record: record.clone(),
                coverage,
            }
        })
        .collect();
    Ok(InstrumentSearchResponse {
        instruments,
        total_matches,
        index_size: index.records.len(),
        indexed_at: index.indexed_at.clone(),
    })
}

async fn search_instrument_catalog(
    State(state): State<AppState>,
    Query(query): Query<InstrumentQuery>,
) -> Result<Json<InstrumentSearchResponse>, ApiError> {
    let worker_state = state.clone();
    let response =
        tokio::task::spawn_blocking(move || instrument_search_blocking(&worker_state, &query))
            .await
            .map_err(|error| anyhow::anyhow!("instrument search task failed: {error}"))??;
    Ok(Json(response))
}

/// Accepts `AAPL`, `aapl.us`, or ` BRK-B ` and returns the bare EODHD US code.
const SDK_SKELETON: &str = include_str!("../../docs/templates/sdk_strategy_skeleton.rs");
const SDK_PLATFORM_KEYS: &[&str] = &[
    "symbols",
    "resolution",
    "session",
    "position_percent",
    "min_price",
    "initial_capital",
    "max_entries_per_day",
    "max_open_positions",
    "max_gross_exposure",
    "tie_break",
    "random_seed",
];

/// Expands `universe:stocks`, `universe:etfs`, and `universe:all` into symbol lists from the
/// EODHD universe files; plain symbols pass through.
fn expand_sdk_universe(state: &AppState, values: &[serde_json::Value]) -> Result<Vec<String>> {
    let data = &state.local.data;
    let stock_universe = data.stock_universe();
    let etf_universe = data.etf_universe();
    let mut symbols = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut push = |symbol: String| {
        if seen.insert(symbol.clone()) {
            symbols.push(symbol);
        }
    };
    for value in values {
        let text = value.as_str().context("symbols must be strings")?;
        let lowered = text.trim().to_ascii_lowercase();
        if let Some(universe) = lowered.strip_prefix("universe:") {
            let files: Vec<&Path> = match universe {
                "stocks" | "us_common_stocks" | "common" => vec![stock_universe.as_path()],
                "etfs" | "us_etfs" => vec![etf_universe.as_path()],
                "all" | "stocks_and_etfs" => {
                    vec![stock_universe.as_path(), etf_universe.as_path()]
                }
                other => bail!(
                    "unknown universe {other:?}; use universe:stocks, universe:etfs, or universe:all"
                ),
            };
            for file in files {
                let text = fs::read_to_string(file)
                    .with_context(|| format!("failed to read universe file {}", file.display()))?;
                for line in text.lines() {
                    let line = line.trim();
                    if !line.is_empty() && !line.starts_with('#') {
                        push(normalize_sdk_symbol(line)?);
                    }
                }
            }
        } else {
            push(normalize_sdk_symbol(text)?);
        }
    }
    Ok(symbols)
}

fn default_engine_path(state: &AppState) -> PathBuf {
    state.local.engine_path(&state.root)
}

fn engine_path_for(state: &AppState, strategy_id: &str) -> Result<PathBuf> {
    let stored: Option<String> = {
        let connection = state.database.lock().expect("database lock poisoned");
        connection
            .query_row(
                "SELECT engine_path FROM strategies WHERE id=?1",
                [strategy_id],
                |row| row.get(0),
            )
            .unwrap_or(None)
    };
    Ok(match stored {
        Some(relative) => state.root.join(checked_workspace_relative(&relative)?),
        None => default_engine_path(state),
    })
}

/// Runs `tessera sdk-manifests` on the given engine, cached by binary mtime.
fn sdk_manifests_for_engine(state: &AppState, engine: &Path) -> Result<Vec<SdkManifest>> {
    anyhow::ensure!(
        engine.is_file(),
        "tessera engine is not built at {}; run cargo build --release --bin tessera",
        engine.display()
    );
    let modified = fs::metadata(engine)?.modified()?;
    {
        let cache = state.sdk_manifests.lock().expect("manifest cache poisoned");
        if let Some((stamp, manifests)) = cache.get(engine) {
            if *stamp == modified {
                return Ok(manifests.clone());
            }
        }
    }
    let output = std::process::Command::new(engine)
        .current_dir(&state.root)
        .arg("sdk-manifests")
        .output()
        .with_context(|| format!("failed to run {}", engine.display()))?;
    anyhow::ensure!(
        output.status.success(),
        "sdk-manifests failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let manifests: Vec<SdkManifest> = serde_json::from_slice(&output.stdout)
        .context("engine returned unreadable SDK manifests")?;
    state
        .sdk_manifests
        .lock()
        .expect("manifest cache poisoned")
        .insert(engine.to_path_buf(), (modified, manifests.clone()));
    Ok(manifests)
}

fn sdk_manifest_for(state: &AppState, strategy: &StrategyRecord) -> Result<SdkManifest> {
    let manifest_id = strategy
        .sdk_strategy_id
        .clone()
        .unwrap_or_else(|| strategy.id.clone());
    let engine = engine_path_for(state, &strategy.id)?;
    sdk_manifests_for_engine(state, &engine)?
        .into_iter()
        .find(|manifest| manifest.id == manifest_id)
        .with_context(|| {
            format!(
                "the engine at {} does not contain strategy {manifest_id:?}",
                engine.display()
            )
        })
}

fn sdk_manifest_id_from_paths(paths: &[String]) -> Result<String> {
    paths
        .iter()
        .filter_map(|path| {
            let path = Path::new(path);
            (path.starts_with("src/strategies/user") && path.extension().is_some_and(|e| e == "rs"))
                .then(|| path.file_stem().and_then(|s| s.to_str()).map(str::to_owned))
                .flatten()
        })
        .next()
        .context("the draft does not contain a src/strategies/user/*.rs strategy file")
}

/// Registers (or refreshes) catalog rows for every SDK strategy an engine exposes.
/// `dev_draft` registers `<id>__dev` rows that point at a draft's freshly built engine.
fn sync_sdk_strategies(
    state: &AppState,
    engine: Option<&Path>,
    dev_draft: Option<&str>,
) -> Result<Vec<String>> {
    let engine = engine
        .map(Path::to_path_buf)
        .unwrap_or_else(|| default_engine_path(state));
    let mut manifests = sdk_manifests_for_engine(state, &engine)?;
    if let Some(draft_id) = dev_draft {
        // A dev build only registers the strategies the draft itself contains.
        let owned = load_draft_files(state, draft_id)?
            .iter()
            .filter_map(|file| {
                Path::new(&file.path)
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .map(str::to_owned)
            })
            .collect::<Vec<_>>();
        manifests.retain(|manifest| owned.contains(&manifest.id));
    }
    let now = Utc::now().to_rfc3339();
    let connection = state.database.lock().expect("database lock poisoned");
    let mut ids = Vec::new();
    for manifest in manifests {
        let row_id = match dev_draft {
            Some(_) => format!("{}__dev", manifest.id),
            None => manifest.id.clone(),
        };
        let name = match dev_draft {
            Some(_) => format!("{} (dev build)", manifest.name),
            None => manifest.name.clone(),
        };
        let status = if dev_draft.is_some() {
            "Dev build"
        } else {
            "Research"
        };
        let source_paths =
            serde_json::to_string(&[format!("src/strategies/user/{}.rs", manifest.id)])?;
        let (engine_path, bundle_path): (Option<String>, Option<String>) = match dev_draft {
            Some(draft_id) => (
                Some(relative_to_root(&state.root, &engine)),
                Some(relative_to_root(&state.root, &draft_root(state, draft_id))),
            ),
            None => (None, None),
        };
        connection.execute(
            "INSERT INTO strategies
             (id, name, version, status, description, asset_scope, config_path, command_name,
              runnable, created_at, base_strategy_id, source_paths_json, source_bundle_path,
              engine_path, sdk_strategy_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'sdk', 'run-strategy', 1, ?7, 'sdk', ?8, ?9, ?10, ?11)
             ON CONFLICT(id) DO UPDATE SET
               name=excluded.name, version=excluded.version, status=excluded.status,
               description=excluded.description, asset_scope=excluded.asset_scope,
               runnable=1, base_strategy_id='sdk', source_paths_json=excluded.source_paths_json,
               source_bundle_path=excluded.source_bundle_path, engine_path=excluded.engine_path,
               sdk_strategy_id=excluded.sdk_strategy_id",
            params![
                row_id,
                name,
                manifest.version,
                status,
                manifest.description,
                manifest.asset_scope,
                now,
                source_paths,
                bundle_path,
                engine_path,
                manifest.id
            ],
        )?;
        ids.push(row_id);
    }
    Ok(ids)
}

fn validate_sdk_platform_parameters(parameters: &serde_json::Value) -> Result<()> {
    let object = parameters
        .as_object()
        .context("parameters must be an object")?;
    if let Some(symbols) = object.get("symbols") {
        let list = symbols.as_array().context("symbols must be a list")?;
        anyhow::ensure!(!list.is_empty(), "select at least one symbol");
    }
    if let Some(resolution) = object.get("resolution").and_then(serde_json::Value::as_str) {
        SdkResolution::parse(resolution)?;
    }
    if let Some(exposure) = object.get("max_gross_exposure") {
        let value = exposure
            .as_f64()
            .context("max_gross_exposure must be a number")?;
        anyhow::ensure!(
            value.is_finite() && value > 0.0,
            "max gross exposure must be a positive multiple of equity"
        );
    }
    if let Some(min_price) = object.get("min_price") {
        let value = min_price.as_f64().context("min_price must be a number")?;
        anyhow::ensure!(
            value.is_finite() && value >= 0.0,
            "minimum price must be zero or positive"
        );
    }
    if let Some(session) = object.get("session").and_then(serde_json::Value::as_str) {
        anyhow::ensure!(
            ["regular", "extended"].contains(&session),
            "session must be regular or extended"
        );
    }
    if let Some(value) = object
        .get("position_percent")
        .and_then(serde_json::Value::as_f64)
    {
        anyhow::ensure!(
            (0.01..=10.0).contains(&value),
            "position percent must be between 0.01 and 10 (times equity)"
        );
    }
    if let Some(value) = object
        .get("initial_capital")
        .and_then(serde_json::Value::as_f64)
    {
        anyhow::ensure!(value > 0.0, "initial capital must be positive");
    }
    if let Some(value) = object.get("tie_break").and_then(serde_json::Value::as_str) {
        anyhow::ensure!(
            ["priority", "random", "alphabetical"].contains(&value),
            "tie_break must be priority, random, or alphabetical"
        );
    }
    Ok(())
}

fn normalize_sdk_symbol(value: &str) -> Result<String> {
    let symbol = value.trim().to_ascii_uppercase();
    anyhow::ensure!(!symbol.is_empty(), "symbol cannot be empty");
    anyhow::ensure!(
        symbol.len() <= 40
            && symbol
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '^' | '_')),
        "symbol {value:?} contains unsupported characters"
    );
    Ok(if symbol.contains('.') {
        symbol
    } else {
        format!("{symbol}.US")
    })
}

/// Builds the frozen SDK run config from form parameters, the manifest, and the cost profile.
fn build_sdk_run_config(
    state: &AppState,
    strategy: &StrategyRecord,
    parameters: &serde_json::Value,
    profile: Option<&CostProfileRecord>,
) -> Result<SdkRunConfig> {
    let manifest = sdk_manifest_for(state, strategy)?;
    let object = parameters.as_object().cloned().unwrap_or_default();
    let requested = object
        .get("symbols")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_else(|| vec![serde_json::json!("SPY.US")]);
    let symbols = expand_sdk_universe(state, &requested)?;
    anyhow::ensure!(!symbols.is_empty(), "select at least one symbol");
    // No symbol cap: standard-mode strategies load every selected symbol's bars up front,
    // so very large explicit lists at intraday resolution trade memory for convenience.
    // Screened-universe strategies stream candidates instead.
    let _ = manifest.screen_universe;
    let limits = SdkLimitsConfig {
        max_entries_per_day: object
            .get("max_entries_per_day")
            .and_then(serde_json::Value::as_u64)
            .map(|value| value as usize)
            .filter(|value| *value > 0),
        max_open_positions: object
            .get("max_open_positions")
            .and_then(serde_json::Value::as_u64)
            .map(|value| value as usize)
            .filter(|value| *value > 0),
        max_gross_exposure: object
            .get("max_gross_exposure")
            .and_then(serde_json::Value::as_f64)
            .filter(|value| value.is_finite() && *value > 0.0),
        tie_break: object
            .get("tie_break")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("priority")
            .to_owned(),
        seed: object
            .get("random_seed")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0),
    };
    let resolution = SdkResolution::parse(
        object
            .get("resolution")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("daily"),
    )?;
    let session = match object.get("session").and_then(serde_json::Value::as_str) {
        Some("extended") => SdkSessionKind::Extended,
        _ => SdkSessionKind::Regular,
    };
    let position_percent = object
        .get("position_percent")
        .and_then(serde_json::Value::as_f64)
        .unwrap_or(1.0);
    let initial_capital = object
        .get("initial_capital")
        .and_then(serde_json::Value::as_f64)
        .unwrap_or(100_000.0);
    let min_price = object
        .get("min_price")
        .and_then(serde_json::Value::as_f64)
        .unwrap_or(1.0);
    let strategy_parameters: serde_json::Map<String, serde_json::Value> = object
        .iter()
        .filter(|(key, _)| !SDK_PLATFORM_KEYS.contains(&key.as_str()))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    manifest.resolve(&strategy_parameters)?;
    let costs = match profile {
        None => SdkCostConfig {
            tick_size: 0.01,
            entry_slippage_ticks: 1,
            exit_slippage_ticks: 1,
            commission_per_unit_per_fill: 0.005,
            all_in_round_trip_bps: None,
            max_commission_percent_of_notional: Some(1.0),
        },
        Some(profile) if profile.model == "none" => SdkCostConfig::default(),
        Some(profile) if profile.model == "fixed_tick_per_unit" => SdkCostConfig {
            tick_size: profile.tick_size,
            entry_slippage_ticks: profile.entry_slippage_ticks,
            exit_slippage_ticks: profile.exit_slippage_ticks,
            commission_per_unit_per_fill: profile
                .entry_commission_per_unit
                .max(profile.exit_commission_per_unit),
            all_in_round_trip_bps: None,
            max_commission_percent_of_notional: Some(1.0),
        },
        Some(profile) => SdkCostConfig {
            tick_size: if profile.tick_size > 0.0 {
                profile.tick_size
            } else {
                0.01
            },
            entry_slippage_ticks: 0,
            exit_slippage_ticks: 0,
            commission_per_unit_per_fill: 0.0,
            all_in_round_trip_bps: Some(profile.entry_bps + profile.exit_bps),
            max_commission_percent_of_notional: Some(1.0),
        },
    };
    let config = SdkRunConfig {
        strategy: strategy
            .sdk_strategy_id
            .clone()
            .unwrap_or_else(|| strategy.id.clone()),
        data: SdkDataConfig {
            resolution,
            session,
            daily_dir: state.local.data.daily_dir.clone(),
            five_minute_dir: state.local.data.five_minute_dir.clone(),
            one_minute_dir: state.local.data.one_minute_dir.clone(),
            symbols,
            calendar_symbol: state.local.data.calendar_symbol.clone(),
        },
        sizing: SdkSizingConfig {
            initial_capital,
            position_percent,
            min_price,
        },
        costs,
        limits,
        parameters: strategy_parameters,
    };
    config.validate()?;
    if manifest.screen_universe {
        anyhow::ensure!(
            resolution != SdkResolution::Daily,
            "{} screens daily bars and trades intraday; choose 5m or 1m bars",
            manifest.name
        );
        let present = config
            .data
            .symbols
            .iter()
            .filter(|symbol| config.daily_file(symbol).is_file())
            .count();
        anyhow::ensure!(present > 0, "none of the selected symbols have daily data");
    } else {
        for symbol in &config.data.symbols {
            anyhow::ensure!(
                config.symbol_file(symbol).is_file(),
                "{} data is not available for {symbol}",
                resolution.label()
            );
            if manifest.daily_context && resolution != SdkResolution::Daily {
                anyhow::ensure!(
                    config.daily_file(symbol).is_file(),
                    "daily data is not available for {symbol} (needed for daily context)"
                );
            }
        }
    }
    Ok(config)
}

fn validate_sdk_id(value: &str) -> Result<()> {
    anyhow::ensure!(
        (3..=48).contains(&value.len())
            && value.chars().next().is_some_and(|c| c.is_ascii_lowercase())
            && value
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'),
        "strategy id must be snake_case: lowercase letters, digits, and underscores (3 to 48 chars)"
    );
    Ok(())
}

fn pascal_case(id: &str) -> String {
    id.split('_')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            let first = chars
                .next()
                .map(|c| c.to_ascii_uppercase())
                .unwrap_or_default();
            format!("{first}{}", chars.as_str())
        })
        .collect()
}

/// Creates a draft holding one skeleton strategy file under src/strategies/user/.
fn create_sdk_draft(
    state: &AppState,
    request: &CreateStrategyDraftRequest,
) -> Result<StrategyDraftDetail> {
    let strategy_id = request
        .strategy_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .context("a snake_case strategy id is required for a new SDK strategy")?;
    validate_sdk_id(strategy_id)?;
    let existing = default_engine_path(state)
        .is_file()
        .then(|| sdk_manifests_for_engine(state, &default_engine_path(state)).ok())
        .flatten()
        .unwrap_or_default();
    anyhow::ensure!(
        !existing.iter().any(|manifest| manifest.id == strategy_id),
        "an SDK strategy with id {strategy_id:?} is already compiled in; pick another id"
    );
    let relative = format!("src/strategies/user/{strategy_id}.rs");
    anyhow::ensure!(
        !state.root.join(&relative).exists(),
        "{relative} already exists in the repository"
    );
    let content = SDK_SKELETON
        .replace("__STRATEGY_ID__", strategy_id)
        .replace("__STRATEGY_NAME__", request.name.trim())
        .replace("__STRUCT_NAME__", &pascal_case(strategy_id));
    let now = Utc::now();
    let id = format!("draft-{}", now.format("%Y%m%dT%H%M%S%.6fZ"));
    let root = draft_root(state, &id);
    let path = root.join(checked_workspace_relative(&relative)?);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&path, &content)?;
    let files = vec![StrategySourceFile {
        path: relative.clone(),
        content,
        editable: true,
    }];
    let hash = hash_source_files(&files);
    {
        let connection = state.database.lock().expect("database lock poisoned");
        connection.execute(
            "INSERT INTO strategy_drafts
             (id, base_strategy_id, name, version, description, status, source_paths_json,
              source_sha256, created_at, updated_at)
             VALUES (?1, 'sdk', ?2, ?3, ?4, 'draft', ?5, ?6, ?7, ?7)",
            params![
                id,
                request.name.trim(),
                request.version.trim(),
                request.description.trim(),
                serde_json::to_string(&[relative])?,
                hash,
                now.to_rfc3339()
            ],
        )?;
    }
    load_draft_detail(state, &id)
}

#[derive(Debug, Serialize)]
struct SdkBuildResponse {
    draft_id: String,
    engine_path: String,
    strategies: Vec<StrategyRecord>,
    log: String,
}

/// Compiles a draft into its own engine and registers `<id>__dev` catalog entries for it.
async fn build_strategy_draft(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<SdkBuildResponse>, ApiError> {
    let draft = load_draft(&state, &id)?;
    require_api(
        draft.base_strategy_id == "sdk",
        "dev builds are available for SDK strategy drafts only",
    )?;
    require_api(draft.status != "released", "released drafts are immutable")?;
    let build_id = format!("devbuild-{}", Utc::now().format("%Y%m%dT%H%M%S%.6fZ"));
    let checkout = prepare_draft_checkout(&state, &id, &build_id)?;
    let target = state.root.join("target/strategy-releases");
    let _permit = state.workers.acquire().await?;
    let output = Command::new("cargo")
        .current_dir(&checkout)
        .env("CARGO_TARGET_DIR", &target)
        .arg("build")
        .arg("--release")
        .arg("--bin")
        .arg("tessera")
        .output()
        .await?;
    let log = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let _ = fs::remove_dir_all(&checkout);
    require_api(
        output.status.success(),
        format!(
            "dev build failed:\n{}",
            log.lines()
                .rev()
                .take(60)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect::<Vec<_>>()
                .join("\n")
        ),
    )?;
    let engine_dir = draft_root(&state, &id).join("engine");
    fs::create_dir_all(&engine_dir)?;
    let engine_path = engine_dir.join("tessera");
    fs::copy(target.join("release/tessera"), &engine_path)?;
    let ids = sync_sdk_strategies(&state, Some(&engine_path), Some(&id))?;
    let strategies = {
        let connection = state.database.lock().expect("database lock poisoned");
        ids.iter()
            .map(|row_id| query_strategy(&connection, row_id))
            .collect::<Result<Vec<_>>>()?
    };
    Ok(Json(SdkBuildResponse {
        draft_id: id,
        engine_path: relative_to_root(&state.root, &engine_path),
        strategies,
        log: log
            .lines()
            .rev()
            .take(20)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join("\n"),
    }))
}

fn execution_family(strategy: &StrategyRecord) -> &str {
    strategy.base_strategy_id.as_deref().unwrap_or(&strategy.id)
}

fn checked_workspace_relative(path: &str) -> Result<&Path> {
    let relative = Path::new(path);
    anyhow::ensure!(relative.is_relative(), "source path must be relative");
    anyhow::ensure!(
        relative
            .components()
            .all(|component| matches!(component, std::path::Component::Normal(_))),
        "source path contains an unsupported component"
    );
    Ok(relative)
}

fn source_bundle_root(state: &AppState, strategy: &StrategyRecord) -> Result<PathBuf> {
    let relative: Option<String> = {
        let connection = state.database.lock().expect("database lock poisoned");
        connection.query_row(
            "SELECT source_bundle_path FROM strategies WHERE id=?1",
            [&strategy.id],
            |row| row.get(0),
        )?
    };
    let Some(relative) = relative else {
        return Ok(state.root.clone());
    };
    let relative = checked_workspace_relative(&relative)?;
    let candidate = state.root.join(relative);
    let releases = state.root.join("strategy_workspace/releases");
    let drafts = state.root.join("strategy_workspace/drafts");
    anyhow::ensure!(
        candidate.starts_with(&releases) || candidate.starts_with(&drafts),
        "source bundle is outside the strategy workspace"
    );
    anyhow::ensure!(candidate.is_dir(), "source bundle directory is missing");
    Ok(candidate)
}

/// Source paths recorded on the catalog row when present, else the family default.
fn strategy_source_paths(state: &AppState, strategy: &StrategyRecord) -> Result<Vec<String>> {
    let stored: Option<String> = {
        let connection = state.database.lock().expect("database lock poisoned");
        connection.query_row(
            "SELECT source_paths_json FROM strategies WHERE id=?1",
            [&strategy.id],
            |row| row.get(0),
        )?
    };
    if let Some(text) = stored {
        if let Ok(paths) = serde_json::from_str::<Vec<String>>(&text) {
            if !paths.is_empty() {
                return Ok(paths);
            }
        }
    }
    Ok(source_paths_for_family(execution_family(strategy))?
        .into_iter()
        .map(str::to_owned)
        .collect())
}

fn load_strategy_source_bundle(
    state: &AppState,
    strategy: &StrategyRecord,
) -> Result<Vec<StrategySourceFile>> {
    let root = source_bundle_root(state, strategy)?;
    strategy_source_paths(state, strategy)?
        .into_iter()
        .map(|relative| {
            let checked = checked_workspace_relative(&relative)?;
            let path = resolve_strategy_source(state, &root, checked)
                .with_context(|| format!("strategy source file is missing: {relative}"))?;
            Ok(StrategySourceFile {
                path: relative.clone(),
                content: fs::read_to_string(path)?,
                editable: false,
            })
        })
        .collect()
}

/// Finds the on-disk file behind a catalog source path. Strategies compiled in from
/// `local.toml` `[strategies] dirs` (for example a private repository) keep the virtual
/// `src/strategies/user/<id>.rs` path in the catalog so drafts, releases, and snapshots
/// place them where `build.rs` discovers them; the file itself is looked up in the extra
/// directories, last directory winning to mirror the build-script override order.
fn resolve_strategy_source(state: &AppState, root: &Path, relative: &Path) -> Result<PathBuf> {
    let direct = root.join(relative);
    if direct.is_file() {
        return Ok(direct);
    }
    let Some(file_name) = relative.file_name() else {
        bail!("source path has no file name");
    };
    let in_user_dir = relative
        .parent()
        .is_some_and(|parent| parent == Path::new("src/strategies/user"));
    if in_user_dir {
        for dir in state.local.strategies.dirs.iter().rev() {
            let candidate = dir.join(file_name);
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }
    bail!(
        "not found under {} or the configured strategy directories",
        root.display()
    )
}

fn hash_source_files(files: &[StrategySourceFile]) -> String {
    let mut ordered = files.iter().collect::<Vec<_>>();
    ordered.sort_by(|left, right| left.path.cmp(&right.path));
    let mut hasher = Sha256::new();
    for file in ordered {
        hasher.update(file.path.as_bytes());
        hasher.update([0]);
        hasher.update(file.content.as_bytes());
        hasher.update([0xff]);
    }
    format!("{:x}", hasher.finalize())
}

async fn strategy_source(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<StrategySourceResponse>, ApiError> {
    let strategy = {
        let connection = state.database.lock().expect("database lock poisoned");
        query_strategy(&connection, &id)?
    };
    let files = load_strategy_source_bundle(&state, &strategy)?;
    let source_sha256 = strategy
        .source_sha256
        .clone()
        .unwrap_or_else(|| hash_source_files(&files));
    Ok(Json(StrategySourceResponse {
        strategy,
        files,
        source_sha256,
        immutable: true,
    }))
}

fn validate_draft_metadata(name: &str, version: &str, description: &str) -> Result<()> {
    anyhow::ensure!(
        (1..=100).contains(&name.trim().len()),
        "name must contain 1 to 100 characters"
    );
    anyhow::ensure!(
        (1..=40).contains(&version.trim().len()),
        "version must contain 1 to 40 characters"
    );
    anyhow::ensure!(
        (1..=300).contains(&description.trim().len()),
        "description must contain 1 to 300 characters"
    );
    Ok(())
}

fn require_api(condition: bool, message: impl Into<String>) -> Result<(), ApiError> {
    if condition {
        Ok(())
    } else {
        Err(anyhow::anyhow!(message.into()).into())
    }
}

fn draft_root(state: &AppState, id: &str) -> PathBuf {
    state.root.join("strategy_workspace/drafts").join(id)
}

fn load_draft_files(state: &AppState, id: &str) -> Result<Vec<StrategySourceFile>> {
    let paths_json: String = {
        let connection = state.database.lock().expect("database lock poisoned");
        connection.query_row(
            "SELECT source_paths_json FROM strategy_drafts WHERE id=?1",
            [id],
            |row| row.get(0),
        )?
    };
    let paths: Vec<String> = serde_json::from_str(&paths_json)?;
    let root = draft_root(state, id);
    paths
        .into_iter()
        .map(|relative| {
            let checked = checked_workspace_relative(&relative)?;
            let path = root.join(checked);
            anyhow::ensure!(
                path.starts_with(&root) && path.is_file(),
                "draft source file is missing"
            );
            Ok(StrategySourceFile {
                path: relative,
                content: fs::read_to_string(path)?,
                editable: true,
            })
        })
        .collect()
}

fn map_draft(row: &rusqlite::Row<'_>) -> rusqlite::Result<StrategyDraftRecord> {
    Ok(StrategyDraftRecord {
        id: row.get(0)?,
        base_strategy_id: row.get(1)?,
        name: row.get(2)?,
        version: row.get(3)?,
        description: row.get(4)?,
        status: row.get(5)?,
        source_sha256: row.get(6)?,
        created_at: row.get(7)?,
        updated_at: row.get(8)?,
        last_validation_id: row.get(9)?,
        release_strategy_id: row.get(10)?,
    })
}

fn load_draft(state: &AppState, id: &str) -> Result<StrategyDraftRecord> {
    let connection = state.database.lock().expect("database lock poisoned");
    Ok(connection.query_row(
        "SELECT id, base_strategy_id, name, version, description, status, source_sha256,
                created_at, updated_at, last_validation_id, release_strategy_id
         FROM strategy_drafts WHERE id=?1",
        [id],
        map_draft,
    )?)
}

fn load_drafts(state: &AppState) -> Result<Vec<StrategyDraftRecord>> {
    let connection = state.database.lock().expect("database lock poisoned");
    let mut statement = connection.prepare(
        "SELECT id, base_strategy_id, name, version, description, status, source_sha256,
                created_at, updated_at, last_validation_id, release_strategy_id
         FROM strategy_drafts ORDER BY updated_at DESC",
    )?;
    let rows = statement.query_map([], map_draft)?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

fn map_validation(
    state: &AppState,
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<StrategyValidationRecord> {
    let log_path: String = row.get(8)?;
    let log = fs::read_to_string(state.root.join(&log_path)).unwrap_or_default();
    Ok(StrategyValidationRecord {
        id: row.get(0)?,
        draft_id: row.get(1)?,
        action: row.get(2)?,
        status: row.get(3)?,
        source_sha256: row.get(4)?,
        created_at: row.get(5)?,
        started_at: row.get(6)?,
        finished_at: row.get(7)?,
        log,
        error: row.get(9)?,
    })
}

fn load_validation(state: &AppState, id: &str) -> Result<StrategyValidationRecord> {
    let connection = state.database.lock().expect("database lock poisoned");
    Ok(connection.query_row(
        "SELECT id, draft_id, action, status, source_sha256, created_at, started_at,
                finished_at, log_path, error FROM strategy_validations WHERE id=?1",
        [id],
        |row| map_validation(state, row),
    )?)
}

fn load_draft_detail(state: &AppState, id: &str) -> Result<StrategyDraftDetail> {
    let draft = load_draft(state, id)?;
    let validation = draft
        .last_validation_id
        .as_deref()
        .map(|validation_id| load_validation(state, validation_id))
        .transpose()?;
    Ok(StrategyDraftDetail {
        draft,
        files: load_draft_files(state, id)?,
        validation,
    })
}

async fn list_strategy_drafts(
    State(state): State<AppState>,
) -> Result<Json<Vec<StrategyDraftRecord>>, ApiError> {
    Ok(Json(load_drafts(&state)?))
}

async fn strategy_draft_detail(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<StrategyDraftDetail>, ApiError> {
    Ok(Json(load_draft_detail(&state, &id)?))
}

async fn create_strategy_draft(
    State(state): State<AppState>,
    Json(request): Json<CreateStrategyDraftRequest>,
) -> Result<(StatusCode, Json<StrategyDraftDetail>), ApiError> {
    validate_draft_metadata(&request.name, &request.version, &request.description)?;
    if request.base_strategy_id == "sdk" {
        let detail = create_sdk_draft(&state, &request)?;
        return Ok((StatusCode::CREATED, Json(detail)));
    }
    let selected = {
        let connection = state.database.lock().expect("database lock poisoned");
        query_strategy(&connection, &request.base_strategy_id)?
    };
    let family = execution_family(&selected).to_owned();
    let source = load_strategy_source_bundle(&state, &selected)?;
    let now = Utc::now();
    let id = format!("draft-{}", now.format("%Y%m%dT%H%M%S%.6fZ"));
    let root = draft_root(&state, &id);
    for file in &source {
        let path = root.join(checked_workspace_relative(&file.path)?);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, &file.content)?;
    }
    let paths = source
        .iter()
        .map(|file| file.path.clone())
        .collect::<Vec<_>>();
    let hash = hash_source_files(&source);
    {
        let connection = state.database.lock().expect("database lock poisoned");
        connection.execute(
            "INSERT INTO strategy_drafts
             (id, base_strategy_id, name, version, description, status, source_paths_json,
              source_sha256, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, 'draft', ?6, ?7, ?8, ?8)",
            params![
                id,
                family,
                request.name.trim(),
                request.version.trim(),
                request.description.trim(),
                serde_json::to_string(&paths)?,
                hash,
                now.to_rfc3339()
            ],
        )?;
    }
    Ok((StatusCode::CREATED, Json(load_draft_detail(&state, &id)?)))
}

async fn save_strategy_draft_file(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
    Json(request): Json<SaveStrategyDraftFileRequest>,
) -> Result<Json<StrategyDraftDetail>, ApiError> {
    let draft = load_draft(&state, &id)?;
    require_api(draft.status != "released", "released drafts are immutable")?;
    require_api(
        request.content.len() <= 2_000_000,
        "source file exceeds the two-megabyte editor limit",
    )?;
    let paths_json: String = {
        let connection = state.database.lock().expect("database lock poisoned");
        connection.query_row(
            "SELECT source_paths_json FROM strategy_drafts WHERE id=?1",
            [&id],
            |row| row.get(0),
        )?
    };
    let paths: Vec<String> = serde_json::from_str(&paths_json)?;
    require_api(
        paths.contains(&request.path),
        "this file is not part of the draft source bundle",
    )?;
    let root = draft_root(&state, &id);
    let path = root.join(checked_workspace_relative(&request.path)?);
    require_api(path.starts_with(&root), "draft path escapes its workspace")?;
    fs::write(path, request.content)?;
    let files = load_draft_files(&state, &id)?;
    let hash = hash_source_files(&files);
    {
        let connection = state.database.lock().expect("database lock poisoned");
        connection.execute(
            "UPDATE strategy_drafts SET source_sha256=?2, updated_at=?3, status='draft',
             last_validation_id=NULL WHERE id=?1",
            params![id, hash, Utc::now().to_rfc3339()],
        )?;
    }
    Ok(Json(load_draft_detail(&state, &id)?))
}

async fn strategy_validation_detail(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<StrategyValidationRecord>, ApiError> {
    Ok(Json(load_validation(&state, &id)?))
}

fn copy_directory(source: &Path, destination: &Path) -> Result<()> {
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_directory(&source_path, &destination_path)?;
        } else {
            fs::copy(&source_path, &destination_path)?;
        }
    }
    Ok(())
}

fn prepare_draft_checkout(state: &AppState, draft_id: &str, build_id: &str) -> Result<PathBuf> {
    let checkout = state.root.join("data/ui/strategy_builds").join(build_id);
    anyhow::ensure!(!checkout.exists(), "validation checkout already exists");
    fs::create_dir_all(&checkout)?;
    for file in ["Cargo.toml", "Cargo.lock", "rustfmt.toml", "build.rs"] {
        let source = state.root.join(file);
        if source.is_file() {
            fs::copy(source, checkout.join(file))?;
        }
    }
    copy_directory(&state.root.join("src"), &checkout.join("src"))?;
    // The UI binary embeds the SDK skeleton template at compile time.
    let templates = state.root.join("docs/templates");
    if templates.is_dir() {
        copy_directory(&templates, &checkout.join("docs/templates"))?;
    }
    for file in load_draft_files(state, draft_id)? {
        let target = checkout.join(checked_workspace_relative(&file.path)?);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(target, file.content)?;
    }
    Ok(checkout)
}

async fn validate_strategy_draft(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
    Json(request): Json<ValidateStrategyDraftRequest>,
) -> Result<(StatusCode, Json<StrategyValidationRecord>), ApiError> {
    require_api(
        ["format", "check", "test"].contains(&request.action.as_str()),
        "validation action must be format, check, or test",
    )?;
    let draft = load_draft(&state, &id)?;
    require_api(draft.status != "released", "released drafts are immutable")?;
    let active: i64 = {
        let connection = state.database.lock().expect("database lock poisoned");
        connection.query_row(
            "SELECT COUNT(*) FROM strategy_validations WHERE draft_id=?1 AND status IN ('queued','running')",
            [&id],
            |row| row.get(0),
        )?
    };
    require_api(active == 0, "this draft already has a validation running")?;
    let now = Utc::now();
    let validation_id = format!("validation-{}", now.format("%Y%m%dT%H%M%S%.6fZ"));
    let log_path = format!("strategy_workspace/logs/{validation_id}.log");
    {
        let connection = state.database.lock().expect("database lock poisoned");
        let transaction = connection.unchecked_transaction()?;
        transaction.execute(
            "INSERT INTO strategy_validations
             (id, draft_id, action, status, source_sha256, created_at, log_path)
             VALUES (?1, ?2, ?3, 'queued', ?4, ?5, ?6)",
            params![
                validation_id,
                id,
                request.action,
                draft.source_sha256,
                now.to_rfc3339(),
                log_path
            ],
        )?;
        transaction.execute(
            "UPDATE strategy_drafts SET last_validation_id=?2, status='validating', updated_at=?3 WHERE id=?1",
            params![id, validation_id, now.to_rfc3339()],
        )?;
        transaction.commit()?;
    }
    let worker_state = state.clone();
    let worker_validation_id = validation_id.clone();
    tokio::spawn(async move {
        if let Err(error) =
            run_strategy_validation(worker_state.clone(), &worker_validation_id).await
        {
            let connection = worker_state
                .database
                .lock()
                .expect("database lock poisoned");
            let _ = connection.execute(
                "UPDATE strategy_validations SET status='failed', finished_at=?2, error=?3 WHERE id=?1",
                params![worker_validation_id, Utc::now().to_rfc3339(), format!("{error:#}")],
            );
        }
    });
    Ok((
        StatusCode::ACCEPTED,
        Json(load_validation(&state, &validation_id)?),
    ))
}

async fn run_strategy_validation(state: AppState, validation_id: &str) -> Result<()> {
    let _permit = state.workers.acquire().await?;
    let validation = load_validation(&state, validation_id)?;
    {
        let connection = state.database.lock().expect("database lock poisoned");
        connection.execute(
            "UPDATE strategy_validations SET status='running', started_at=?2 WHERE id=?1",
            params![validation_id, Utc::now().to_rfc3339()],
        )?;
    }
    let log_path: String = {
        let connection = state.database.lock().expect("database lock poisoned");
        connection.query_row(
            "SELECT log_path FROM strategy_validations WHERE id=?1",
            [validation_id],
            |row| row.get(0),
        )?
    };
    let mut log = Vec::new();
    let mut succeeded = true;
    if validation.action == "format" {
        for file in load_draft_files(&state, &validation.draft_id)? {
            let path = draft_root(&state, &validation.draft_id).join(&file.path);
            let output = Command::new("rustfmt")
                .arg("--edition")
                .arg("2024")
                .arg(&path)
                .output()
                .await?;
            log.extend_from_slice(format!("--- rustfmt {} ---\n", file.path).as_bytes());
            log.extend_from_slice(&output.stdout);
            log.extend_from_slice(&output.stderr);
            succeeded &= output.status.success();
        }
    } else {
        let checkout = prepare_draft_checkout(&state, &validation.draft_id, validation_id)?;
        // Reuse the repository target directory so draft validation shares the
        // already-compiled dependency graph instead of consuming several extra
        // gigabytes per local workspace.
        let target = state.root.join("target");
        let commands: Vec<(&str, Vec<&str>)> = if validation.action == "test" {
            vec![
                ("format check", vec!["fmt", "--", "--check"]),
                ("compile", vec!["check", "--all-targets"]),
                ("unit tests", vec!["test", "--lib"]),
            ]
        } else {
            vec![
                ("format check", vec!["fmt", "--", "--check"]),
                ("compile", vec!["check", "--all-targets"]),
            ]
        };
        for (label, arguments) in commands {
            let output = Command::new("cargo")
                .current_dir(&checkout)
                .env("CARGO_TARGET_DIR", &target)
                .args(arguments)
                .output()
                .await?;
            log.extend_from_slice(format!("--- {label} ---\n").as_bytes());
            log.extend_from_slice(&output.stdout);
            log.extend_from_slice(&output.stderr);
            if !output.status.success() {
                succeeded = false;
                break;
            }
        }
    }
    let log_file = state.root.join(&log_path);
    if let Some(parent) = log_file.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&log_file, log)?;
    let files = load_draft_files(&state, &validation.draft_id)?;
    let source_sha256 = hash_source_files(&files);
    let status = if succeeded { "complete" } else { "failed" };
    let draft_status = if succeeded {
        match validation.action.as_str() {
            "test" => "validated",
            "check" => "compiled",
            _ => "draft",
        }
    } else {
        "draft"
    };
    let error = (!succeeded).then_some(format!(
        "{} failed; inspect the validation log",
        validation.action
    ));
    let connection = state.database.lock().expect("database lock poisoned");
    let transaction = connection.unchecked_transaction()?;
    transaction.execute(
        "UPDATE strategy_validations SET status=?2, source_sha256=?3, finished_at=?4, error=?5 WHERE id=?1",
        params![validation_id, status, source_sha256, Utc::now().to_rfc3339(), error],
    )?;
    transaction.execute(
        "UPDATE strategy_drafts SET status=?2, source_sha256=?3, updated_at=?4 WHERE id=?1",
        params![
            validation.draft_id,
            draft_status,
            source_sha256,
            Utc::now().to_rfc3339()
        ],
    )?;
    transaction.commit()?;
    Ok(())
}

fn validate_strategy_slug(value: &str) -> Result<()> {
    anyhow::ensure!(
        (3..=80).contains(&value.len()),
        "strategy id must contain 3 to 80 characters"
    );
    anyhow::ensure!(
        value
            .chars()
            .next()
            .is_some_and(|character| character.is_ascii_lowercase()),
        "strategy id must start with a lowercase letter"
    );
    anyhow::ensure!(
        value.chars().all(|character| character.is_ascii_lowercase()
            || character.is_ascii_digit()
            || character == '_'),
        "strategy id may contain lowercase letters, numbers, and underscores only"
    );
    Ok(())
}

async fn release_strategy_draft(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
    Json(request): Json<ReleaseStrategyDraftRequest>,
) -> Result<(StatusCode, Json<StrategySourceResponse>), ApiError> {
    validate_strategy_slug(&request.strategy_id)?;
    validate_draft_metadata(&request.name, &request.version, &request.description)?;
    let draft = load_draft(&state, &id)?;
    require_api(
        draft.status == "validated",
        "run the full test action against the current source before release",
    )?;
    let validation_id = draft
        .last_validation_id
        .as_deref()
        .context("validated draft has no validation record")?;
    let validation = load_validation(&state, validation_id)?;
    require_api(
        validation.action == "test" && validation.status == "complete",
        "the latest validation must be a completed test run",
    )?;
    require_api(
        validation.source_sha256 == draft.source_sha256,
        "source changed after validation; test it again before release",
    )?;
    let strategy_exists: i64 = {
        let connection = state.database.lock().expect("database lock poisoned");
        connection.query_row(
            "SELECT COUNT(*) FROM strategies WHERE id=?1",
            [&request.strategy_id],
            |row| row.get(0),
        )?
    };
    require_api(
        strategy_exists == 0,
        "strategy id already exists; releases are immutable",
    )?;

    let release_suffix = Utc::now().format("%Y%m%dT%H%M%S%.6fZ").to_string();
    let checkout = prepare_draft_checkout(&state, &id, &format!("release-{release_suffix}"))?;
    let target = state.root.join("target/strategy-releases");
    let _permit = state.workers.acquire().await?;
    let output = Command::new("cargo")
        .current_dir(&checkout)
        .env("CARGO_TARGET_DIR", &target)
        .arg("build")
        .arg("--release")
        .arg("--bin")
        .arg("tessera")
        .output()
        .await?;
    require_api(
        output.status.success(),
        format!(
            "release build failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        ),
    )?;

    let version_slug = request
        .version
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect::<String>();
    let release_root = state
        .root
        .join("strategy_workspace/releases")
        .join(&request.strategy_id)
        .join(version_slug);
    require_api(
        !release_root.exists(),
        "this strategy version already has a release bundle",
    )?;
    fs::create_dir_all(&release_root)?;
    for file in load_draft_files(&state, &id)? {
        let target_file = release_root.join(checked_workspace_relative(&file.path)?);
        if let Some(parent) = target_file.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(target_file, file.content)?;
    }
    let engine_path = release_root.join("tessera");
    fs::copy(target.join("release/tessera"), &engine_path)?;
    let paths = load_draft_files(&state, &id)?
        .iter()
        .map(|file| file.path.clone())
        .collect::<Vec<_>>();
    let (asset_scope, config_path, sdk_strategy_id) = if draft.base_strategy_id == "sdk" {
        let manifest_id = sdk_manifest_id_from_paths(&paths)?;
        let manifests = sdk_manifests_for_engine(&state, &engine_path)?;
        let manifest = manifests
            .iter()
            .find(|manifest| manifest.id == manifest_id)
            .with_context(|| format!("release engine does not contain manifest {manifest_id:?}"))?;
        (
            manifest.asset_scope.clone(),
            "sdk".to_owned(),
            Some(manifest_id),
        )
    } else {
        let base = {
            let connection = state.database.lock().expect("database lock poisoned");
            query_strategy(&connection, &draft.base_strategy_id)?
        };
        (base.asset_scope, base.config_path, None)
    };
    let released_at = Utc::now().to_rfc3339();
    {
        let connection = state.database.lock().expect("database lock poisoned");
        let transaction = connection.unchecked_transaction()?;
        transaction.execute(
            "INSERT INTO strategies
             (id, name, version, status, description, asset_scope, config_path, command_name,
              runnable, created_at, base_strategy_id, source_paths_json, source_sha256,
              source_bundle_path, engine_path, released_at)
             VALUES (?1, ?2, ?3, 'Research', ?4, ?5, ?6,
                     COALESCE((SELECT command_name FROM strategies WHERE id=?7), 'run-strategy'), 1, ?8, ?7, ?9, ?10, ?11, ?12, ?8)",
            params![
                request.strategy_id,
                request.name.trim(),
                request.version.trim(),
                request.description.trim(),
                asset_scope,
                config_path,
                draft.base_strategy_id,
                released_at,
                serde_json::to_string(&paths)?,
                draft.source_sha256,
                relative_to_root(&state.root, &release_root),
                relative_to_root(&state.root, &engine_path)
            ],
        )?;
        if let Some(manifest_id) = &sdk_strategy_id {
            transaction.execute(
                "UPDATE strategies SET sdk_strategy_id=?2 WHERE id=?1",
                params![request.strategy_id, manifest_id],
            )?;
        }
        transaction.execute(
            "UPDATE strategy_drafts SET status='released', release_strategy_id=?2, updated_at=?3 WHERE id=?1",
            params![id, request.strategy_id, released_at],
        )?;
        transaction.commit()?;
    }
    let strategy = {
        let connection = state.database.lock().expect("database lock poisoned");
        query_strategy(&connection, &request.strategy_id)?
    };
    let files = load_strategy_source_bundle(&state, &strategy)?;
    Ok((
        StatusCode::CREATED,
        Json(StrategySourceResponse {
            source_sha256: draft.source_sha256,
            strategy,
            files,
            immutable: true,
        }),
    ))
}

async fn list_cost_profiles(
    State(state): State<AppState>,
) -> Result<Json<Vec<CostProfileRecord>>, ApiError> {
    Ok(Json(load_cost_profiles(&state)?))
}

async fn create_cost_profile(
    State(state): State<AppState>,
    Json(request): Json<CreateCostProfileRequest>,
) -> Result<(StatusCode, Json<CostProfileRecord>), ApiError> {
    validate_cost_profile_request(&request)?;
    let now = Utc::now();
    let id = format!("cost-{}", now.format("%Y%m%dT%H%M%S%.6fZ"));
    {
        let connection = state.database.lock().expect("database lock poisoned");
        connection.execute(
            "INSERT INTO cost_profiles
             (id, name, asset_class, model, entry_bps, exit_bps, tick_size,
              entry_slippage_ticks, exit_slippage_ticks, entry_commission_per_unit,
              exit_commission_per_unit, minimum_commission, created_at, builtin, immutable)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, 0, 1)",
            params![
                id,
                request.name.trim(),
                request.asset_class,
                request.model,
                request.entry_bps,
                request.exit_bps,
                request.tick_size,
                request.entry_slippage_ticks,
                request.exit_slippage_ticks,
                request.entry_commission_per_unit,
                request.exit_commission_per_unit,
                request.minimum_commission,
                now.to_rfc3339()
            ],
        )?;
    }
    Ok((StatusCode::CREATED, Json(load_cost_profile(&state, &id)?)))
}

async fn list_automations(
    State(state): State<AppState>,
) -> Result<Json<Vec<AutomationScheduleRecord>>, ApiError> {
    Ok(Json(load_automations(&state)?))
}

async fn create_automation(
    State(state): State<AppState>,
    Json(request): Json<CreateAutomationScheduleRequest>,
) -> Result<(StatusCode, Json<AutomationScheduleRecord>), ApiError> {
    validate_automation_request(&request)?;
    let now = Utc::now();
    let id = format!("automation-{}", now.format("%Y%m%dT%H%M%S%.6fZ"));
    {
        let connection = state.database.lock().expect("database lock poisoned");
        connection.execute(
            "INSERT INTO automation_schedules
             (id, name, kind, enabled, local_time, weekdays, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                id,
                request.name.trim(),
                request.kind,
                request.enabled,
                request.local_time,
                request.weekdays,
                now.to_rfc3339()
            ],
        )?;
    }
    Ok((StatusCode::CREATED, Json(load_automation(&state, &id)?)))
}

async fn toggle_automation(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<AutomationScheduleRecord>, ApiError> {
    {
        let connection = state.database.lock().expect("database lock poisoned");
        let changed = connection.execute(
            "UPDATE automation_schedules SET enabled=CASE enabled WHEN 0 THEN 1 ELSE 0 END WHERE id=?1",
            [&id],
        )?;
        if changed != 1 {
            return Err(anyhow::anyhow!("automation was not found").into());
        }
    }
    Ok(Json(load_automation(&state, &id)?))
}

async fn run_automation_now(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<AutomationScheduleRecord>, ApiError> {
    execute_automation(&state, &id).await?;
    Ok(Json(load_automation(&state, &id)?))
}

async fn import_legacy(State(state): State<AppState>) -> Result<Json<ImportResponse>, ApiError> {
    Ok(Json(import_legacy_reports(&state)?))
}

async fn list_jobs(State(state): State<AppState>) -> Result<Json<Vec<JobRecord>>, ApiError> {
    Ok(Json(load_jobs(&state, 25)?))
}

async fn list_runs(State(state): State<AppState>) -> Result<Json<Vec<RunRecord>>, ApiError> {
    let connection = state.database.lock().expect("database lock poisoned");
    Ok(Json(query_runs(&connection, 250)?))
}

async fn strategy_detail(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<StrategyDetailResponse>, ApiError> {
    let (strategy, runs) = {
        let connection = state.database.lock().expect("database lock poisoned");
        (
            query_strategy(&connection, &id)?,
            query_strategy_runs(&connection, &id, 250)?,
        )
    };
    let presets = load_presets(&state, &id)?;
    let family = execution_family(&strategy).to_owned();
    let sdk_manifest: Option<SdkManifest>;
    let (mut rules, default_parameters) = match family.as_str() {
        "sdk" => {
            let manifest = sdk_manifest_for(&state, &strategy)?;
            let mut defaults = serde_json::Value::Object(manifest.defaults());
            defaults["symbols"] = serde_json::json!(["SPY.US"]);
            defaults["resolution"] = serde_json::json!("daily");
            defaults["session"] = serde_json::json!("regular");
            defaults["position_percent"] = serde_json::json!(1.0);
            defaults["initial_capital"] = serde_json::json!(100_000.0);
            defaults["max_entries_per_day"] =
                serde_json::json!(manifest.default_max_entries_per_day.unwrap_or(0));
            defaults["max_open_positions"] = serde_json::json!(0);
            if let Some(exposure) = manifest.default_max_gross_exposure {
                defaults["max_gross_exposure"] = serde_json::json!(exposure);
            }
            defaults["tie_break"] = serde_json::json!(
                manifest
                    .default_tie_break
                    .clone()
                    .unwrap_or_else(|| "priority".to_owned())
            );
            defaults["random_seed"] = serde_json::json!(manifest.default_seed);
            if !manifest.default_symbols.is_empty() {
                defaults["symbols"] = serde_json::json!(manifest.default_symbols);
            } else if manifest.screen_universe {
                defaults["symbols"] = serde_json::json!(["universe:stocks"]);
            }
            if let Some(resolution) = &manifest.default_resolution {
                defaults["resolution"] = serde_json::json!(resolution);
            } else if manifest.screen_universe {
                defaults["resolution"] = serde_json::json!("5m");
            }
            let mut rules = vec![manifest.description.clone()];
            rules.extend(manifest.rules.iter().cloned());
            rules.push(format!(
                "Replays {} warm-up bars before the requested start so indicators are ready; orders during warm-up are ignored.",
                manifest.warmup_bars
            ));
            sdk_manifest = Some(manifest);
            (rules, defaults)
        }
        _ => {
            return Err(anyhow::anyhow!(
                "this strategy is not runnable in this build; only SDK strategies can run"
            )
            .into());
        }
    };
    if strategy.custom {
        rules.push(format!(
            "This immutable custom release was compiled from source SHA-256 {} and executes through the {} adapter.",
            strategy.source_sha256.as_deref().unwrap_or("unavailable"),
            family
        ));
    }
    Ok(Json(StrategyDetailResponse {
        instruments: instrument_requirement(&family),
        sdk: sdk_manifest,
        strategy,
        rules,
        default_parameters,
        presets,
        runs,
    }))
}

async fn save_preset(
    State(state): State<AppState>,
    Json(request): Json<SavePresetRequest>,
) -> Result<(StatusCode, Json<PresetRecord>), ApiError> {
    let strategy = load_runnable_strategy(&state, &request.strategy_id)?;
    let name = request.name.trim();
    if name.is_empty() || name.len() > 80 {
        return Err(anyhow::anyhow!("preset name must contain 1 to 80 characters").into());
    }
    validate_strategy_parameters(execution_family(&strategy), &request.parameters)?;
    let created_at = Utc::now().to_rfc3339();
    let id = format!("preset-{}", Utc::now().format("%Y%m%dT%H%M%S%.6fZ"));
    let parameters_json = serde_json::to_string(&request.parameters)?;
    {
        let connection = state.database.lock().expect("database lock poisoned");
        connection.execute(
            "INSERT INTO strategy_presets
             (id, strategy_id, name, parameters_json, costs_enabled, created_at, immutable)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1)",
            params![
                id,
                request.strategy_id,
                name,
                parameters_json,
                request.costs_enabled,
                created_at
            ],
        )?;
    }
    Ok((
        StatusCode::CREATED,
        Json(PresetRecord {
            id,
            strategy_id: request.strategy_id,
            name: name.to_owned(),
            parameters: request.parameters,
            costs_enabled: request.costs_enabled,
            created_at,
        }),
    ))
}

async fn run_detail(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<RunDetailResponse>, ApiError> {
    // Parquet reads for a universe-sized run take seconds; keep them off the async
    // runtime so dashboard polling and other requests stay responsive meanwhile.
    let response = tokio::task::spawn_blocking(move || run_detail_sync(&state, &id))
        .await
        .context("run detail task failed")??;
    Ok(Json(response))
}

fn run_detail_sync(state: &AppState, id: &str) -> Result<RunDetailResponse> {
    let (run, job_error) = {
        let connection = state.database.lock().expect("database lock poisoned");
        let run = query_run(&connection, id)?;
        let job_error: Option<String> = connection
            .query_row("SELECT error FROM jobs WHERE run_id = ?1", [id], |row| {
                row.get::<_, Option<String>>(0)
            })
            .optional()?
            .flatten();
        (run, job_error)
    };
    let artifact_dir = checked_artifact_path(&state.root, &run.artifact_dir)?;
    let report_path = run
        .report_path
        .as_ref()
        .map(|path| checked_artifact_path(&state.root, path))
        .transpose()?;
    let report = if artifact_dir.join("run_config.toml").is_file()
        && artifact_dir.join("daily_equity.parquet").is_file()
        && artifact_dir.join("trades.parquet").is_file()
        && artifact_dir.join("coverage.parquet").is_file()
    {
        load_report_view(&artifact_dir).ok()
    } else {
        None
    };
    let config_path = run
        .config_path
        .as_ref()
        .map(|path| state.root.join(path))
        .filter(|path| path.is_file())
        .or_else(|| {
            artifact_dir
                .join("run_config.toml")
                .is_file()
                .then(|| artifact_dir.join("run_config.toml"))
        });
    let config_text = config_path.and_then(|path| fs::read_to_string(path).ok());
    let manifest = fs::read_to_string(artifact_dir.join("run_manifest.json"))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok());
    let report_url = report_path
        .filter(|path| path.is_file())
        .map(|_| format!("/api/runs/{id}/report"));
    Ok(RunDetailResponse {
        run,
        report,
        report_url,
        config_text,
        manifest,
        job_error,
    })
}

async fn run_report(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Result<Html<String>, ApiError> {
    let report_path = {
        let connection = state.database.lock().expect("database lock poisoned");
        connection.query_row("SELECT report_path FROM runs WHERE id=?1", [&id], |row| {
            row.get::<_, Option<String>>(0)
        })?
    }
    .context("this run does not have an HTML report")?;
    let report_path = checked_artifact_path(&state.root, &report_path)?;
    Ok(Html(fs::read_to_string(report_path)?))
}

async fn data_status(State(state): State<AppState>) -> Result<Json<DataStatusResponse>, ApiError> {
    Ok(Json(load_data_status(&state)?))
}

async fn start_eod_update(
    State(state): State<AppState>,
) -> Result<(StatusCode, Json<DataUpdateRecord>), ApiError> {
    let record = queue_eod_update(&state)?;
    Ok((StatusCode::ACCEPTED, Json(record)))
}

fn queue_eod_update(state: &AppState) -> Result<DataUpdateRecord> {
    let active = {
        let connection = state.database.lock().expect("database lock poisoned");
        connection.query_row(
            "SELECT COUNT(*) FROM data_updates WHERE status IN ('queued', 'running')",
            [],
            |row| row.get::<_, i64>(0),
        )?
    };
    if active != 0 {
        return Err(anyhow::anyhow!("a US EOD update is already running").into());
    }
    let now = Utc::now();
    let id = format!("data-{}", now.format("%Y%m%dT%H%M%S%.6fZ"));
    let log_path = format!("data/ui/logs/{id}.log");
    {
        let connection = state.database.lock().expect("database lock poisoned");
        connection.execute(
            "INSERT INTO data_updates (id, status, created_at, log_path)
             VALUES (?1, 'queued', ?2, ?3)",
            params![id, now.to_rfc3339(), log_path],
        )?;
    }
    let worker_state = state.clone();
    let worker_id = id.clone();
    tokio::spawn(async move {
        if let Err(error) = run_eod_update(worker_state.clone(), &worker_id).await {
            let connection = worker_state
                .database
                .lock()
                .expect("database lock poisoned");
            let _ = connection.execute(
                "UPDATE data_updates SET status='failed', finished_at=?2, error=?3 WHERE id=?1",
                params![worker_id, Utc::now().to_rfc3339(), format!("{error:#}")],
            );
        }
    });
    load_data_update(state, &id)
}

fn load_data_status(state: &AppState) -> Result<DataStatusResponse> {
    let data = &state.local.data;
    let freshness: Option<serde_json::Value> = data
        .freshness_file
        .as_ref()
        .and_then(|path| fs::read_to_string(path).ok())
        .and_then(|text| serde_json::from_str(&text).ok());
    let latest_calendar_date =
        last_csv_date(&data.daily_dir.join(format!("{}.csv", data.calendar_symbol)))
            .unwrap_or_else(|| "unknown".to_owned());
    let latest_market_date = freshness
        .as_ref()
        .and_then(|value| value.get("last_market_date"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| latest_calendar_date.clone());
    let daily_files = fs::read_dir(&data.daily_dir)
        .map(|entries| {
            entries
                .flatten()
                .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "csv"))
                .count()
        })
        .unwrap_or(0);
    let update_job = {
        let connection = state.database.lock().expect("database lock poisoned");
        connection
            .query_row(
                "SELECT id, status, created_at, started_at, finished_at, log_path, error
                 FROM data_updates ORDER BY created_at DESC LIMIT 1",
                [],
                map_data_update,
            )
            .optional()?
    };
    Ok(DataStatusResponse {
        latest_market_date,
        latest_spy_date: latest_calendar_date,
        symbols_on_latest_date: freshness
            .as_ref()
            .and_then(|value| value.get("eligible_rows"))
            .and_then(serde_json::Value::as_u64)
            .map(|value| value as usize)
            .unwrap_or(daily_files),
        universe_symbols: freshness
            .as_ref()
            .and_then(|value| value.get("universe_symbols"))
            .and_then(serde_json::Value::as_u64)
            .map(|value| value as usize)
            .unwrap_or(daily_files),
        updated_at_utc: freshness
            .as_ref()
            .and_then(|value| value.get("updated_at_utc"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unknown")
            .to_owned(),
        update_job,
    })
}

fn last_csv_date(path: &Path) -> Option<String> {
    fs::read_to_string(path).ok().and_then(|text| {
        text.lines().rev().find_map(|line| {
            let value = line.split(',').next()?;
            NaiveDate::parse_from_str(value, "%Y-%m-%d")
                .ok()
                .map(|date| date.to_string())
        })
    })
}

fn map_data_update(row: &rusqlite::Row<'_>) -> rusqlite::Result<DataUpdateRecord> {
    Ok(DataUpdateRecord {
        id: row.get(0)?,
        status: row.get(1)?,
        created_at: row.get(2)?,
        started_at: row.get(3)?,
        finished_at: row.get(4)?,
        log_path: row.get(5)?,
        error: row.get(6)?,
    })
}

fn load_data_update(state: &AppState, id: &str) -> Result<DataUpdateRecord> {
    let connection = state.database.lock().expect("database lock poisoned");
    Ok(connection.query_row(
        "SELECT id, status, created_at, started_at, finished_at, log_path, error
         FROM data_updates WHERE id=?1",
        [id],
        map_data_update,
    )?)
}

async fn run_eod_update(state: AppState, id: &str) -> Result<()> {
    let started_at = Utc::now().to_rfc3339();
    {
        let connection = state.database.lock().expect("database lock poisoned");
        connection.execute(
            "UPDATE data_updates SET status='running', started_at=?2 WHERE id=?1",
            params![id, started_at],
        )?;
    }
    let command = state
        .local
        .data
        .update_command
        .clone()
        .context("no update_command is configured in local.toml for this data library")?;
    let update = load_data_update(&state, id)?;
    let output = Command::new("/bin/sh")
        .current_dir(&state.root)
        .arg("-c")
        .arg(&command)
        .output()
        .await?;
    let mut log = format!("--- {command} ---\n--- stdout ---\n").into_bytes();
    log.extend_from_slice(&output.stdout);
    log.extend_from_slice(b"\n--- stderr ---\n");
    log.extend_from_slice(&output.stderr);
    let log_path = state.root.join(&update.log_path);
    if let Some(parent) = log_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&log_path, &log)?;
    let finished_at = Utc::now().to_rfc3339();
    let connection = state.database.lock().expect("database lock poisoned");
    if output.status.success() {
        connection.execute(
            "UPDATE data_updates SET status='complete', finished_at=?2 WHERE id=?1",
            params![id, finished_at],
        )?;
    } else {
        let error = format!(
            "data update command exited with code {}; inspect {}",
            output.status.code().unwrap_or(-1),
            update.log_path
        );
        connection.execute(
            "UPDATE data_updates SET status='failed', finished_at=?2, error=?3 WHERE id=?1",
            params![id, finished_at, error],
        )?;
    }
    Ok(())
}

async fn create_job(
    State(state): State<AppState>,
    Json(request): Json<CreateJobRequest>,
) -> Result<(StatusCode, Json<JobRecord>), ApiError> {
    let strategy = load_runnable_strategy(&state, &request.strategy_id)?;
    let mut validation_request = request.clone();
    validation_request.strategy_id = execution_family(&strategy).to_owned();
    validate_job_request(&validation_request)?;
    verify_instrument_data(&state, &strategy, &request.parameters)?;
    let job = insert_queued_job(&state, &request)?;
    let job_for_response = load_job(&state, &job)?;
    let worker_state = state.clone();
    tokio::spawn(async move {
        if let Err(error) = run_job(worker_state.clone(), job.clone()).await {
            let _ = mark_job_failed(&worker_state, &job, &format!("{error:#}"));
        }
    });
    Ok((StatusCode::ACCEPTED, Json(job_for_response)))
}

async fn list_sweeps(State(state): State<AppState>) -> Result<Json<Vec<SweepRecord>>, ApiError> {
    Ok(Json(load_sweeps(&state)?))
}

async fn sweep_detail(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<SweepDetailResponse>, ApiError> {
    let detail = tokio::task::spawn_blocking(move || load_sweep_detail(&state, &id))
        .await
        .context("sweep detail task failed")??;
    Ok(Json(detail))
}

async fn create_sweep(
    State(state): State<AppState>,
    Json(request): Json<CreateSweepRequest>,
) -> Result<(StatusCode, Json<SweepDetailResponse>), ApiError> {
    let strategy = load_runnable_strategy(&state, &request.strategy_id)?;
    let mut validation_request = request.clone();
    validation_request.strategy_id = execution_family(&strategy).to_owned();
    validate_sweep_request(&validation_request)?;
    let combinations = expand_sweep_parameters(&request.base_parameters, &request.axes)?;
    let now = Utc::now();
    let sweep_id = format!("sweep-{}", now.format("%Y%m%dT%H%M%S%.6fZ"));
    {
        let connection = state.database.lock().expect("database lock poisoned");
        connection.execute(
            "INSERT INTO sweeps
             (id, strategy_id, name, research_label, start_date, end_date, axes_json,
              costs_enabled, created_at, immutable)
             VALUES (?1, ?2, ?3, 'Development', ?4, ?5, ?6, ?7, ?8, 1)",
            params![
                sweep_id,
                request.strategy_id,
                request.name.trim(),
                request.start_date,
                request.end_date,
                serde_json::to_string(&request.axes)?,
                request.costs_enabled,
                now.to_rfc3339()
            ],
        )?;
    }

    let mut job_ids = Vec::with_capacity(combinations.len());
    for (index, parameters) in combinations.into_iter().enumerate() {
        let job_request = CreateJobRequest {
            strategy_id: request.strategy_id.clone(),
            start_date: request.start_date.clone(),
            end_date: request.end_date.clone(),
            research_label: "Development".to_owned(),
            name: Some(format!("{} · config {:02}", request.name.trim(), index + 1)),
            parameters: parameters.clone(),
            costs_enabled: request.costs_enabled,
            cost_profile_id: request.cost_profile_id.clone(),
        };
        let job_id = insert_queued_job(&state, &job_request)?;
        let job = load_job(&state, &job_id)?;
        {
            let connection = state.database.lock().expect("database lock poisoned");
            connection.execute(
                "INSERT INTO sweep_members
                 (sweep_id, configuration_index, run_id, job_id, parameters_json)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    sweep_id,
                    index as i64,
                    job.run_id,
                    job.id,
                    serde_json::to_string(&parameters)?
                ],
            )?;
        }
        job_ids.push(job_id);
    }
    for job_id in job_ids {
        let worker_state = state.clone();
        tokio::spawn(async move {
            if let Err(error) = run_job(worker_state.clone(), job_id.clone()).await {
                let _ = mark_job_failed(&worker_state, &job_id, &format!("{error:#}"));
            }
        });
    }
    let detail = tokio::task::spawn_blocking(move || load_sweep_detail(&state, &sweep_id))
        .await
        .context("sweep detail task failed")??;
    Ok((StatusCode::ACCEPTED, Json(detail)))
}

async fn list_portfolios(
    State(state): State<AppState>,
) -> Result<Json<Vec<PortfolioRecord>>, ApiError> {
    Ok(Json(load_portfolios(&state)?))
}

async fn create_portfolio(
    State(state): State<AppState>,
    Json(request): Json<CreatePortfolioRequest>,
) -> Result<(StatusCode, Json<PortfolioRecord>), ApiError> {
    validate_portfolio_request(&request)?;
    let worker_state = state.clone();
    let record =
        tokio::task::spawn_blocking(move || create_portfolio_artifact(&worker_state, &request))
            .await??;
    Ok((StatusCode::CREATED, Json(record)))
}

fn create_portfolio_artifact(
    state: &AppState,
    request: &CreatePortfolioRequest,
) -> Result<PortfolioRecord> {
    let capital_mode = parse_capital_mode(&request.capital_mode)?;
    let source_runs = {
        let connection = state.database.lock().expect("database lock poisoned");
        request
            .components
            .iter()
            .map(|component| query_run(&connection, &component.run_id))
            .collect::<Result<Vec<_>>>()?
    };
    let mut components = Vec::with_capacity(source_runs.len());
    for (source, request_component) in source_runs.iter().zip(&request.components) {
        anyhow::ensure!(
            source.status == "Complete",
            "source run {} is not complete",
            source.id
        );
        let results_dir = checked_artifact_path(&state.root, &source.artifact_dir)?;
        for required in [
            "run_config.toml",
            "daily_equity.parquet",
            "trades.parquet",
            "coverage.parquet",
        ] {
            anyhow::ensure!(
                results_dir.join(required).is_file(),
                "source run {} is missing {}",
                source.id,
                required
            );
        }
        components.push(PortfolioComponentConfig {
            name: source.name.clone(),
            results_dir,
            weight: request_component.weight,
            capital_group: if capital_mode == CapitalMode::SequentialGroups {
                Some(
                    request_component
                        .capital_group
                        .clone()
                        .filter(|group| !group.trim().is_empty())
                        .unwrap_or_else(|| "overlay".to_owned()),
                )
            } else {
                None
            },
        });
    }

    let now = Utc::now();
    let suffix = now.format("%Y%m%dT%H%M%S%.6fZ").to_string();
    let portfolio_id = format!("portfolio-{suffix}");
    let run_id = format!("portfolio-run-{suffix}");
    let artifact_dir_relative = format!("artifacts/ui_portfolios/{portfolio_id}");
    let artifact_dir = state.root.join(&artifact_dir_relative);
    fs::create_dir_all(&artifact_dir)?;
    let input_config = artifact_dir.join("portfolio_input.toml");
    let config = PortfolioConfig {
        initial_capital: request.initial_capital,
        rebalance: RebalanceMethod::Daily,
        capital_mode,
        components,
    };
    fs::write(&input_config, toml::to_string_pretty(&config)?)?;
    let summary = combine_portfolio(&input_config, &artifact_dir)?;
    let report_path = artifact_dir.join("report.html");
    generate_report(&artifact_dir, Some(&report_path))?;
    let created_at = now.to_rfc3339();
    {
        let connection = state.database.lock().expect("database lock poisoned");
        let transaction = connection.unchecked_transaction()?;
        transaction.execute(
            "INSERT INTO runs
             (id, strategy_id, name, research_label, status, legacy, artifact_dir,
              report_path, config_path, start_date, end_date, created_at, immutable, exit_code)
             VALUES (?1, NULL, ?2, 'Portfolio', 'Complete', 0, ?3, ?4, ?5, ?6, ?7, ?8, 1, 0)",
            params![
                run_id,
                request.name.trim(),
                artifact_dir_relative,
                relative_to_root(&state.root, &report_path),
                relative_to_root(&state.root, &artifact_dir.join("run_config.toml")),
                summary.start.to_string(),
                summary.end.to_string(),
                created_at
            ],
        )?;
        transaction.execute(
            "INSERT INTO portfolios
             (id, run_id, name, capital_mode, initial_capital, created_at, immutable)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1)",
            params![
                portfolio_id,
                run_id,
                request.name.trim(),
                request.capital_mode,
                request.initial_capital,
                created_at
            ],
        )?;
        for (index, component) in request.components.iter().enumerate() {
            transaction.execute(
                "INSERT INTO portfolio_components
                 (portfolio_id, component_index, source_run_id, weight, capital_group)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    portfolio_id,
                    index as i64,
                    component.run_id,
                    component.weight,
                    component.capital_group
                ],
            )?;
        }
        transaction.commit()?;
    }
    Ok(PortfolioRecord {
        id: portfolio_id,
        run_id,
        name: request.name.trim().to_owned(),
        capital_mode: request.capital_mode.clone(),
        initial_capital: request.initial_capital,
        created_at,
        component_count: request.components.len(),
    })
}

fn load_dashboard(state: &AppState) -> Result<DashboardResponse> {
    let connection = state.database.lock().expect("database lock poisoned");
    let strategies = query_strategies(&connection)?;
    let recent_runs = query_runs(&connection, 6)?;
    let jobs = attach_progress(&state.root, query_jobs(&connection, 8)?);
    let historical_reports =
        connection.query_row("SELECT COUNT(*) FROM runs WHERE legacy = 1", [], |row| {
            row.get::<_, i64>(0)
        })? as usize;
    let active_jobs = connection.query_row(
        "SELECT COUNT(*) FROM jobs WHERE status IN ('queued', 'running')",
        [],
        |row| row.get::<_, i64>(0),
    )? as usize;
    let production_strategies = strategies
        .iter()
        .filter(|item| item.status == "Production")
        .count();
    Ok(DashboardResponse {
        strategies,
        recent_runs,
        jobs,
        production_strategies,
        historical_reports,
        active_jobs,
        worker_capacity: 2,
    })
}

fn query_strategies(connection: &Connection) -> Result<Vec<StrategyRecord>> {
    let mut statement = connection.prepare(
        "SELECT id, name, version, status, description, asset_scope, config_path, runnable,
                base_strategy_id, source_sha256, sdk_strategy_id
         FROM strategies WHERE id != 'sdk' ORDER BY name",
    )?;
    let rows = statement.query_map([], |row| {
        Ok(StrategyRecord {
            id: row.get(0)?,
            name: row.get(1)?,
            version: row.get(2)?,
            status: row.get(3)?,
            description: row.get(4)?,
            asset_scope: row.get(5)?,
            config_path: row.get(6)?,
            runnable: row.get::<_, i64>(7)? != 0,
            base_strategy_id: row.get(8)?,
            source_sha256: row.get(9)?,
            custom: row
                .get::<_, Option<String>>(8)?
                .is_some_and(|base| base != "sdk"),
            sdk_strategy_id: row.get(10)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

fn query_strategy(connection: &Connection, id: &str) -> Result<StrategyRecord> {
    Ok(connection.query_row(
        "SELECT id, name, version, status, description, asset_scope, config_path, runnable,
                base_strategy_id, source_sha256, sdk_strategy_id
         FROM strategies WHERE id=?1",
        [id],
        |row| {
            Ok(StrategyRecord {
                id: row.get(0)?,
                name: row.get(1)?,
                version: row.get(2)?,
                status: row.get(3)?,
                description: row.get(4)?,
                asset_scope: row.get(5)?,
                config_path: row.get(6)?,
                runnable: row.get::<_, i64>(7)? != 0,
                base_strategy_id: row.get(8)?,
                source_sha256: row.get(9)?,
                custom: row
                    .get::<_, Option<String>>(8)?
                    .is_some_and(|base| base != "sdk"),
                sdk_strategy_id: row.get(10)?,
            })
        },
    )?)
}

fn parse_metrics_json(text: Option<String>) -> Option<RunMetrics> {
    text.and_then(|text| {
        serde_json::from_str::<Option<RunMetrics>>(&text)
            .ok()
            .flatten()
    })
}

/// Reads headline metrics from a result directory: the parquet report contract first, then the
/// legacy `summary.json`. Returns `None` when neither is available.
fn compute_metrics_for_dir(dir: &Path) -> Option<RunMetrics> {
    if dir.join("run_config.toml").is_file()
        && dir.join("daily_equity.parquet").is_file()
        && dir.join("trades.parquet").is_file()
        && dir.join("coverage.parquet").is_file()
    {
        if let Ok(view) = load_report_view(dir) {
            return Some(RunMetrics {
                cagr_percent: Some(view.metrics.cagr_percent),
                total_return_percent: Some(view.metrics.total_return_percent),
                sharpe: view.metrics.sharpe,
                sortino: view.metrics.sortino,
                calmar: view.metrics.calmar,
                max_drawdown_percent: Some(view.metrics.max_drawdown_percent),
                annual_volatility_percent: Some(view.metrics.annual_volatility_percent),
                win_rate_percent: Some(view.metrics.win_rate_percent),
                trades: Some(view.trades.len()),
                start: Some(view.start.to_string()),
                end: Some(view.end.to_string()),
            });
        }
    }
    let summary: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(dir.join("summary.json")).ok()?).ok()?;
    let number = |key: &str| summary.get(key).and_then(serde_json::Value::as_f64);
    let text = |key: &str| {
        summary
            .get(key)
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned)
    };
    Some(RunMetrics {
        cagr_percent: number("cagr_percent"),
        total_return_percent: None,
        sharpe: number("sharpe"),
        sortino: number("sortino"),
        calmar: None,
        max_drawdown_percent: number("maximum_drawdown_percent"),
        annual_volatility_percent: number("annualized_volatility_percent"),
        win_rate_percent: None,
        trades: summary
            .get("trades")
            .and_then(serde_json::Value::as_u64)
            .map(|value| value as usize),
        start: text("start"),
        end: text("end"),
    })
}

/// Computes and caches metrics for completed runs that have never been summarized. The database
/// lock is released while report files are read so the API stays responsive during a backfill.
fn backfill_run_metrics(
    state: &AppState,
    strategy_id: Option<&str>,
    limit: usize,
) -> Result<usize> {
    let pending: Vec<(String, String)> = {
        let connection = state.database.lock().expect("database lock poisoned");
        let mut statement = connection.prepare(
            "SELECT id, artifact_dir FROM runs
             WHERE status = 'Complete' AND metrics_json IS NULL
               AND (?1 IS NULL OR strategy_id = ?1)
             ORDER BY created_at DESC
             LIMIT ?2",
        )?;
        let rows = statement.query_map(
            params![strategy_id, limit.min(i64::MAX as usize) as i64],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    let mut updated = 0;
    for (id, artifact_dir) in pending {
        let metrics = checked_artifact_path(&state.root, &artifact_dir)
            .ok()
            .and_then(|dir| compute_metrics_for_dir(&dir));
        let json = serde_json::to_string(&metrics)?;
        let connection = state.database.lock().expect("database lock poisoned");
        connection.execute(
            "UPDATE runs SET metrics_json = ?2 WHERE id = ?1 AND metrics_json IS NULL",
            params![id, json],
        )?;
        updated += 1;
    }
    Ok(updated)
}

async fn set_run_star(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
    Json(request): Json<StarRequest>,
) -> Result<Json<RunRecord>, ApiError> {
    let connection = state.database.lock().expect("database lock poisoned");
    let changed = connection.execute(
        "UPDATE runs SET starred = ?2 WHERE id = ?1",
        params![id, i64::from(request.starred)],
    )?;
    if changed != 1 {
        return Err(anyhow::anyhow!("run {id} was not found").into());
    }
    Ok(Json(query_run(&connection, &id)?))
}

fn query_runs(connection: &Connection, limit: usize) -> Result<Vec<RunRecord>> {
    let mut statement = connection.prepare(
        "SELECT id, strategy_id, name, research_label, status, legacy, report_path, artifact_dir,
                config_path, start_date, end_date, created_at, starred, metrics_json
         FROM runs ORDER BY created_at DESC LIMIT ?1",
    )?;
    let rows = statement.query_map([limit as i64], |row| {
        Ok(RunRecord {
            id: row.get(0)?,
            strategy_id: row.get(1)?,
            name: row.get(2)?,
            research_label: row.get(3)?,
            status: row.get(4)?,
            legacy: row.get::<_, i64>(5)? != 0,
            report_path: row.get(6)?,
            artifact_dir: row.get(7)?,
            config_path: row.get(8)?,
            start_date: row.get(9)?,
            end_date: row.get(10)?,
            created_at: row.get(11)?,
            starred: row.get::<_, i64>(12)? != 0,
            metrics_cached: row.get::<_, Option<String>>(13)?.is_some(),
            metrics: parse_metrics_json(row.get::<_, Option<String>>(13)?),
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

fn query_strategy_runs(
    connection: &Connection,
    strategy_id: &str,
    limit: usize,
) -> Result<Vec<RunRecord>> {
    let mut statement = connection.prepare(
        "SELECT id, strategy_id, name, research_label, status, legacy, report_path, artifact_dir,
                config_path, start_date, end_date, created_at, starred, metrics_json
         FROM runs
         WHERE strategy_id = ?1
         ORDER BY created_at DESC
         LIMIT ?2",
    )?;
    let rows = statement.query_map(params![strategy_id, limit as i64], |row| {
        Ok(RunRecord {
            id: row.get(0)?,
            strategy_id: row.get(1)?,
            name: row.get(2)?,
            research_label: row.get(3)?,
            status: row.get(4)?,
            legacy: row.get::<_, i64>(5)? != 0,
            report_path: row.get(6)?,
            artifact_dir: row.get(7)?,
            config_path: row.get(8)?,
            start_date: row.get(9)?,
            end_date: row.get(10)?,
            created_at: row.get(11)?,
            starred: row.get::<_, i64>(12)? != 0,
            metrics_cached: row.get::<_, Option<String>>(13)?.is_some(),
            metrics: parse_metrics_json(row.get::<_, Option<String>>(13)?),
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

fn load_sweeps(state: &AppState) -> Result<Vec<SweepRecord>> {
    let connection = state.database.lock().expect("database lock poisoned");
    let mut statement = connection.prepare(
        "SELECT id, strategy_id, name, research_label, start_date, end_date,
                axes_json, costs_enabled, created_at
         FROM sweeps ORDER BY created_at DESC LIMIT 100",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, String>(4)?,
            row.get::<_, String>(5)?,
            row.get::<_, String>(6)?,
            row.get::<_, i64>(7)? != 0,
            row.get::<_, String>(8)?,
        ))
    })?;
    let mut sweeps = Vec::new();
    for row in rows {
        let (
            id,
            strategy_id,
            name,
            research_label,
            start_date,
            end_date,
            axes_json,
            costs_enabled,
            created_at,
        ) = row?;
        let (configuration_count, complete_count, failed_count, running_count) =
            sweep_counts(&connection, &id)?;
        let status = if configuration_count == 0 {
            "Empty"
        } else if complete_count + failed_count == configuration_count {
            if failed_count == 0 {
                "Complete"
            } else {
                "Complete with errors"
            }
        } else if running_count > 0 {
            "Running"
        } else {
            "Queued"
        };
        sweeps.push(SweepRecord {
            id,
            strategy_id,
            name,
            research_label,
            start_date,
            end_date,
            axes: serde_json::from_str(&axes_json)?,
            costs_enabled,
            created_at,
            status: status.to_owned(),
            configuration_count,
            complete_count,
            failed_count,
        });
    }
    Ok(sweeps)
}

fn load_portfolios(state: &AppState) -> Result<Vec<PortfolioRecord>> {
    let connection = state.database.lock().expect("database lock poisoned");
    let mut statement = connection.prepare(
        "SELECT portfolios.id, portfolios.run_id, portfolios.name,
                portfolios.capital_mode, portfolios.initial_capital,
                portfolios.created_at, COUNT(portfolio_components.component_index)
         FROM portfolios
         LEFT JOIN portfolio_components ON portfolio_components.portfolio_id=portfolios.id
         GROUP BY portfolios.id
         ORDER BY portfolios.created_at DESC
         LIMIT 100",
    )?;
    let rows = statement.query_map([], |row| {
        Ok(PortfolioRecord {
            id: row.get(0)?,
            run_id: row.get(1)?,
            name: row.get(2)?,
            capital_mode: row.get(3)?,
            initial_capital: row.get(4)?,
            created_at: row.get(5)?,
            component_count: row.get::<_, i64>(6)? as usize,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

fn sweep_counts(connection: &Connection, sweep_id: &str) -> Result<(usize, usize, usize, usize)> {
    let counts = connection.query_row(
        "SELECT COUNT(*),
                COALESCE(SUM(CASE WHEN jobs.status='complete' THEN 1 ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN jobs.status='failed' THEN 1 ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN jobs.status='running' THEN 1 ELSE 0 END), 0)
         FROM sweep_members JOIN jobs ON jobs.id=sweep_members.job_id
         WHERE sweep_members.sweep_id=?1",
        [sweep_id],
        |row| {
            Ok((
                row.get::<_, i64>(0)? as usize,
                row.get::<_, i64>(1)? as usize,
                row.get::<_, i64>(2)? as usize,
                row.get::<_, i64>(3)? as usize,
            ))
        },
    )?;
    Ok(counts)
}

fn load_sweep_detail(state: &AppState, id: &str) -> Result<SweepDetailResponse> {
    let sweep = {
        let connection = state.database.lock().expect("database lock poisoned");
        let (
            id,
            strategy_id,
            name,
            research_label,
            start_date,
            end_date,
            axes_json,
            costs_enabled,
            created_at,
        ) = connection.query_row(
            "SELECT id, strategy_id, name, research_label, start_date, end_date,
                    axes_json, costs_enabled, created_at
             FROM sweeps WHERE id=?1",
            [id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, i64>(7)? != 0,
                    row.get::<_, String>(8)?,
                ))
            },
        )?;
        let (configuration_count, complete_count, failed_count, running_count) =
            sweep_counts(&connection, &id)?;
        let status = if complete_count + failed_count == configuration_count {
            if failed_count == 0 {
                "Complete"
            } else {
                "Complete with errors"
            }
        } else if running_count > 0 {
            "Running"
        } else {
            "Queued"
        };
        SweepRecord {
            id,
            strategy_id,
            name,
            research_label,
            start_date,
            end_date,
            axes: serde_json::from_str(&axes_json)?,
            costs_enabled,
            created_at,
            status: status.to_owned(),
            configuration_count,
            complete_count,
            failed_count,
        }
    };
    let rows = {
        let connection = state.database.lock().expect("database lock poisoned");
        let mut statement = connection.prepare(
            "SELECT sweep_members.configuration_index, sweep_members.run_id,
                    sweep_members.job_id, jobs.status, sweep_members.parameters_json,
                    runs.artifact_dir
             FROM sweep_members
             JOIN jobs ON jobs.id=sweep_members.job_id
             JOIN runs ON runs.id=sweep_members.run_id
             WHERE sweep_members.sweep_id=?1
             ORDER BY sweep_members.configuration_index",
        )?;
        let mapped = statement.query_map([id], |row| {
            Ok((
                row.get::<_, i64>(0)? as usize,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
            ))
        })?;
        mapped.collect::<rusqlite::Result<Vec<_>>>()?
    };
    let mut members = Vec::with_capacity(rows.len());
    for (configuration_index, run_id, job_id, status, parameters_json, artifact_dir) in rows {
        let report = if status == "complete" {
            checked_artifact_path(&state.root, &artifact_dir)
                .ok()
                .and_then(|path| load_report_view(&path).ok())
        } else {
            None
        };
        members.push(SweepMemberRecord {
            configuration_index,
            run_id,
            job_id,
            status,
            parameters: serde_json::from_str(&parameters_json)?,
            metrics: report.map(|report| SweepMetrics {
                sharpe: report.metrics.sharpe,
                cagr_percent: report.metrics.cagr_percent,
                max_drawdown_percent: report.metrics.max_drawdown_percent,
                annual_volatility_percent: report.metrics.annual_volatility_percent,
                trade_count: report.trades.len(),
            }),
        });
    }
    Ok(SweepDetailResponse { sweep, members })
}

fn query_run(connection: &Connection, id: &str) -> Result<RunRecord> {
    Ok(connection.query_row(
        "SELECT id, strategy_id, name, research_label, status, legacy, report_path, artifact_dir,
                config_path, start_date, end_date, created_at, starred, metrics_json
         FROM runs WHERE id=?1",
        [id],
        |row| {
            Ok(RunRecord {
                id: row.get(0)?,
                strategy_id: row.get(1)?,
                name: row.get(2)?,
                research_label: row.get(3)?,
                status: row.get(4)?,
                legacy: row.get::<_, i64>(5)? != 0,
                report_path: row.get(6)?,
                artifact_dir: row.get(7)?,
                config_path: row.get(8)?,
                start_date: row.get(9)?,
                end_date: row.get(10)?,
                created_at: row.get(11)?,
                starred: row.get::<_, i64>(12)? != 0,
                metrics_cached: row.get::<_, Option<String>>(13)?.is_some(),
                metrics: parse_metrics_json(row.get::<_, Option<String>>(13)?),
            })
        },
    )?)
}

fn load_presets(state: &AppState, strategy_id: &str) -> Result<Vec<PresetRecord>> {
    let connection = state.database.lock().expect("database lock poisoned");
    let mut statement = connection.prepare(
        "SELECT id, strategy_id, name, parameters_json, costs_enabled, created_at
         FROM strategy_presets WHERE strategy_id=?1 ORDER BY created_at DESC",
    )?;
    let rows = statement.query_map([strategy_id], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, i64>(4)? != 0,
            row.get::<_, String>(5)?,
        ))
    })?;
    let mut presets = Vec::new();
    for row in rows {
        let (id, strategy_id, name, parameters_json, costs_enabled, created_at) = row?;
        presets.push(PresetRecord {
            id,
            strategy_id,
            name,
            parameters: serde_json::from_str(&parameters_json)?,
            costs_enabled,
            created_at,
        });
    }
    Ok(presets)
}

fn checked_artifact_path(root: &Path, relative: &str) -> Result<PathBuf> {
    let relative = Path::new(relative);
    anyhow::ensure!(relative.is_relative(), "artifact path must be relative");
    anyhow::ensure!(
        relative
            .components()
            .all(|component| matches!(component, std::path::Component::Normal(_))),
        "artifact path contains an unsupported component"
    );
    let artifacts = root.join("artifacts");
    let candidate = root.join(relative);
    anyhow::ensure!(
        candidate.starts_with(&artifacts),
        "registered artifact path is outside the artifacts directory"
    );
    if candidate.exists() {
        let canonical_artifacts = artifacts.canonicalize()?;
        let canonical_candidate = candidate.canonicalize()?;
        anyhow::ensure!(
            canonical_candidate.starts_with(canonical_artifacts),
            "registered artifact resolves outside the artifacts directory"
        );
        Ok(canonical_candidate)
    } else {
        Ok(candidate)
    }
}

fn query_jobs(connection: &Connection, limit: usize) -> Result<Vec<JobRecord>> {
    let mut statement = connection.prepare(
        "SELECT id, run_id, strategy_id, status, start_date, end_date, created_at,
                started_at, finished_at, log_path, error, parameters_json, costs_enabled
         FROM jobs ORDER BY created_at DESC LIMIT ?1",
    )?;
    let rows = statement.query_map([limit as i64], map_job)?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

fn map_job(row: &rusqlite::Row<'_>) -> rusqlite::Result<JobRecord> {
    Ok(JobRecord {
        id: row.get(0)?,
        run_id: row.get(1)?,
        strategy_id: row.get(2)?,
        status: row.get(3)?,
        start_date: row.get(4)?,
        end_date: row.get(5)?,
        created_at: row.get(6)?,
        started_at: row.get(7)?,
        finished_at: row.get(8)?,
        log_path: row.get(9)?,
        error: row.get(10)?,
        parameters_json: row.get(11)?,
        costs_enabled: row.get::<_, i64>(12)? != 0,
        progress: None,
    })
}

fn import_legacy_reports(state: &AppState) -> Result<ImportResponse> {
    let artifacts = state.root.join("artifacts");
    let mut discovered = 0;
    let mut imported = 0;
    if !artifacts.exists() {
        return Ok(ImportResponse {
            discovered,
            imported,
        });
    }
    let connection = state.database.lock().expect("database lock poisoned");
    for entry in fs::read_dir(&artifacts)? {
        let entry = entry?;
        let artifact_dir = entry.path();
        let report = artifact_dir.join("report.html");
        if !artifact_dir.is_dir() || !report.is_file() {
            continue;
        }
        discovered += 1;
        let folder = entry.file_name().to_string_lossy().into_owned();
        let name = humanize(&folder);
        let created_at = fs::metadata(&report)
            .and_then(|metadata| metadata.modified())
            .map(DateTime::<Utc>::from)
            .unwrap_or_else(|_| Utc::now())
            .to_rfc3339();
        let changed = connection.execute(
            "INSERT OR IGNORE INTO runs
             (id, name, research_label, status, legacy, artifact_dir, report_path, created_at, immutable)
             VALUES (?1, ?2, 'Legacy / unclassified', 'Complete', 1, ?3, ?4, ?5, 1)",
            params![
                format!("legacy:{folder}"),
                name,
                relative_to_root(&state.root, &artifact_dir),
                relative_to_root(&state.root, &report),
                created_at
            ],
        )?;
        imported += changed;
    }
    Ok(ImportResponse {
        discovered,
        imported,
    })
}

fn humanize(folder: &str) -> String {
    folder
        .split('_')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            chars
                .next()
                .map(|first| first.to_uppercase().collect::<String>() + chars.as_str())
                .unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn relative_to_root(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

fn validate_job_request(request: &CreateJobRequest) -> Result<()> {
    ensure_runnable_strategy(&request.strategy_id)?;
    let start = NaiveDate::parse_from_str(&request.start_date, "%Y-%m-%d")
        .context("start_date must use YYYY-MM-DD")?;
    let end = NaiveDate::parse_from_str(&request.end_date, "%Y-%m-%d")
        .context("end_date must use YYYY-MM-DD")?;
    if start > end {
        bail!("start_date must be on or before end_date");
    }
    if ![
        "Development",
        "Validation",
        "Final holdout",
        "Post-selection",
        "Research",
    ]
    .contains(&request.research_label.as_str())
    {
        bail!("unsupported research label");
    }
    validate_strategy_parameters(&request.strategy_id, &request.parameters)?;
    Ok(())
}

fn validate_sweep_request(request: &CreateSweepRequest) -> Result<()> {
    let name = request.name.trim();
    anyhow::ensure!(
        !name.is_empty() && name.len() <= 80,
        "sweep name must contain 1 to 80 characters"
    );
    anyhow::ensure!(
        (1..=2).contains(&request.axes.len()),
        "a sweep must contain one or two parameter axes"
    );
    let mut seen = std::collections::HashSet::new();
    anyhow::ensure!(
        request.strategy_id == "sdk",
        "this strategy is not runnable in this build; only SDK strategies can run"
    );
    // SDK sweep axes are validated against the manifest when each configuration is queued.
    let supported: Option<&[&str]> = None;
    let mut configuration_count = 1usize;
    for axis in &request.axes {
        anyhow::ensure!(
            supported.is_none_or(|list| list.contains(&axis.parameter.as_str())),
            "unsupported sweep parameter: {}",
            axis.parameter
        );
        anyhow::ensure!(
            seen.insert(axis.parameter.as_str()),
            "each sweep axis must use a different parameter"
        );
        anyhow::ensure!(
            (2..=5).contains(&axis.values.len()),
            "each sweep axis must contain 2 to 5 values"
        );
        anyhow::ensure!(
            axis.values.iter().all(serde_json::Value::is_number),
            "initial sweeps support numeric parameter values only"
        );
        configuration_count = configuration_count.saturating_mul(axis.values.len());
    }
    anyhow::ensure!(
        configuration_count <= 25,
        "a sweep may contain at most 25 configurations"
    );
    for parameters in expand_sweep_parameters(&request.base_parameters, &request.axes)? {
        validate_job_request(&CreateJobRequest {
            strategy_id: request.strategy_id.clone(),
            start_date: request.start_date.clone(),
            end_date: request.end_date.clone(),
            research_label: "Development".to_owned(),
            name: Some(name.to_owned()),
            parameters,
            costs_enabled: request.costs_enabled,
            cost_profile_id: request.cost_profile_id.clone(),
        })?;
    }
    Ok(())
}

fn validate_portfolio_request(request: &CreatePortfolioRequest) -> Result<()> {
    let name = request.name.trim();
    anyhow::ensure!(
        !name.is_empty() && name.len() <= 100,
        "portfolio name must contain 1 to 100 characters"
    );
    anyhow::ensure!(
        request.initial_capital.is_finite()
            && (1_000.0..=1_000_000_000.0).contains(&request.initial_capital),
        "initial capital must be between $1,000 and $1 billion"
    );
    let mode = parse_capital_mode(&request.capital_mode)?;
    anyhow::ensure!(
        (2..=8).contains(&request.components.len()),
        "a portfolio must contain 2 to 8 source runs"
    );
    let mut seen = std::collections::HashSet::new();
    for component in &request.components {
        anyhow::ensure!(
            seen.insert(component.run_id.as_str()),
            "a source run can appear only once"
        );
        anyhow::ensure!(
            component.weight.is_finite() && component.weight > 0.0,
            "component weights must be positive"
        );
        if mode != CapitalMode::NormalizedWeights {
            anyhow::ensure!(
                component.weight <= 1.0,
                "full-capital component weights cannot exceed 100%"
            );
        }
    }
    Ok(())
}

fn parse_capital_mode(value: &str) -> Result<CapitalMode> {
    match value {
        "normalized_weights" => Ok(CapitalMode::NormalizedWeights),
        "sequential_full_capital" => Ok(CapitalMode::SequentialFullCapital),
        "unconstrained_overlays" => Ok(CapitalMode::SequentialGroups),
        _ => bail!("unsupported portfolio capital mode"),
    }
}

fn expand_sweep_parameters(
    base_parameters: &serde_json::Value,
    axes: &[SweepAxis],
) -> Result<Vec<serde_json::Value>> {
    let base = base_parameters
        .as_object()
        .context("base_parameters must be a JSON object")?;
    let mut combinations = vec![base.clone()];
    for axis in axes {
        let mut expanded = Vec::with_capacity(combinations.len() * axis.values.len());
        for combination in &combinations {
            for value in &axis.values {
                let mut next = combination.clone();
                next.insert(axis.parameter.clone(), value.clone());
                expanded.push(next);
            }
        }
        combinations = expanded;
    }
    Ok(combinations
        .into_iter()
        .map(serde_json::Value::Object)
        .collect())
}

fn load_runnable_strategy(state: &AppState, strategy_id: &str) -> Result<StrategyRecord> {
    let strategy = {
        let connection = state.database.lock().expect("database lock poisoned");
        query_strategy(&connection, strategy_id)?
    };
    anyhow::ensure!(strategy.runnable, "this strategy is not runnable");
    ensure_runnable_strategy(execution_family(&strategy))?;
    Ok(strategy)
}

fn ensure_runnable_strategy(strategy_id: &str) -> Result<()> {
    anyhow::ensure!(
        strategy_id == "sdk",
        "this strategy is not runnable in this build; only SDK strategies can run"
    );
    Ok(())
}

/// Fails fast at queue time when a requested instrument lacks the data its strategy needs,
/// so the picker's selection is checked before a run record is created.
fn verify_instrument_data(
    state: &AppState,
    strategy: &StrategyRecord,
    parameters: &serde_json::Value,
) -> Result<()> {
    let _config_path = state.root.join(&strategy.config_path);
    match execution_family(strategy) {
        "sdk" => {
            build_sdk_run_config(state, strategy, parameters, None)?;
        }
        _ => {}
    }
    Ok(())
}

fn validate_strategy_parameters(strategy_id: &str, parameters: &serde_json::Value) -> Result<()> {
    match strategy_id {
        "sdk" => validate_sdk_platform_parameters(parameters),
        _ => bail!("this strategy is not runnable in this build; only SDK strategies can run"),
    }
}

fn insert_queued_job(state: &AppState, request: &CreateJobRequest) -> Result<String> {
    let runtime_strategy = load_runnable_strategy(state, &request.strategy_id)?;
    let family = execution_family(&runtime_strategy);
    let now = Utc::now();
    let suffix = now.format("%Y%m%dT%H%M%S%.6fZ").to_string();
    let job_id = format!("job-{suffix}");
    let run_id = format!("run-{suffix}");
    let artifact_slug = match family {
        "sdk" => runtime_strategy.id.to_ascii_lowercase(),
        _ => bail!("this strategy is not runnable in this build; only SDK strategies can run"),
    };
    let artifact_slug = if runtime_strategy.custom {
        request.strategy_id.clone()
    } else {
        artifact_slug
    };
    let artifact_dir = format!("artifacts/ui_runs/{artifact_slug}_{suffix}");
    let log_path = format!("{artifact_dir}/worker.log");
    let config_snapshot = format!("{artifact_dir}/strategy.toml");
    let name = request
        .name
        .clone()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| {
            let strategy_name = if runtime_strategy.custom {
                runtime_strategy.name.clone()
            } else {
                match family {
                    "sdk" => runtime_strategy.name.clone(),
                    _ => "Backtest".to_owned(),
                }
            };
            format!(
                "{strategy_name} · {} to {}",
                request.start_date, request.end_date
            )
        });
    let selected_profile_id = if request.costs_enabled {
        request.cost_profile_id.clone().or_else(|| {
            Some(
                match family {
                    "sdk" => "us-equities-default",
                    _ => "costs-off",
                }
                .to_owned(),
            )
        })
    } else {
        Some("costs-off".to_owned())
    };
    let profile_snapshot = selected_profile_id
        .as_deref()
        .map(|id| load_cost_profile(state, id))
        .transpose()?;
    if let Some(profile) = &profile_snapshot {
        validate_profile_compatibility(family, profile)?;
    }
    let profile_snapshot_json = serde_json::to_string(&profile_snapshot)?;
    let connection = state.database.lock().expect("database lock poisoned");
    let transaction = connection.unchecked_transaction()?;
    let parameters_json = serde_json::to_string(&request.parameters)?;
    transaction.execute(
        "INSERT INTO runs
         (id, strategy_id, name, research_label, status, legacy, artifact_dir, config_path, start_date, end_date, created_at, immutable)
         VALUES (?1, ?2, ?3, ?4, 'Queued', 0, ?5, ?6, ?7, ?8, ?9, 1)",
        params![run_id, request.strategy_id, name, request.research_label, artifact_dir,
            config_snapshot, request.start_date, request.end_date, now.to_rfc3339()],
    )?;
    transaction.execute(
        "INSERT INTO jobs
         (id, run_id, strategy_id, status, start_date, end_date, created_at, log_path,
          parameters_json, costs_enabled, cost_profile_id, cost_profile_snapshot_json)
         VALUES (?1, ?2, ?3, 'queued', ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            job_id,
            run_id,
            request.strategy_id,
            request.start_date,
            request.end_date,
            now.to_rfc3339(),
            log_path,
            parameters_json,
            request.costs_enabled,
            selected_profile_id,
            profile_snapshot_json
        ],
    )?;
    transaction.commit()?;
    Ok(job_id)
}

fn load_job(state: &AppState, job_id: &str) -> Result<JobRecord> {
    let connection = state.database.lock().expect("database lock poisoned");
    Ok(connection.query_row(
        "SELECT id, run_id, strategy_id, status, start_date, end_date, created_at,
                started_at, finished_at, log_path, error, parameters_json, costs_enabled
         FROM jobs WHERE id = ?1",
        [job_id],
        map_job,
    )?)
}

fn load_jobs(state: &AppState, limit: usize) -> Result<Vec<JobRecord>> {
    let connection = state.database.lock().expect("database lock poisoned");
    Ok(attach_progress(
        &state.root,
        query_jobs(&connection, limit)?,
    ))
}

#[derive(Clone, Copy)]
enum JobExecutionPlan {
    Standard(&'static str),
}

async fn run_job(state: AppState, job_id: String) -> Result<()> {
    let _permit = state.workers.acquire().await?;
    let job = load_job(&state, &job_id)?;
    let runtime_strategy = load_runnable_strategy(&state, &job.strategy_id)?;
    let family = execution_family(&runtime_strategy).to_owned();
    let run = {
        let connection = state.database.lock().expect("database lock poisoned");
        connection.query_row(
            "SELECT artifact_dir FROM runs WHERE id = ?1",
            [&job.run_id],
            |row| row.get::<_, String>(0),
        )?
    };
    let output_dir = state.root.join(&run);
    fs::create_dir_all(&output_dir)?;
    let log_path = state.root.join(&job.log_path);
    let config_snapshot = output_dir.join("strategy.toml");
    let parameters: serde_json::Value = serde_json::from_str(&job.parameters_json)?;
    let cost_profile: Option<CostProfileRecord> = {
        let connection = state.database.lock().expect("database lock poisoned");
        let raw: String = connection.query_row(
            "SELECT cost_profile_snapshot_json FROM jobs WHERE id=?1",
            [&job.id],
            |row| row.get(0),
        )?;
        serde_json::from_str(&raw)?
    };
    let (plan, engine_name) = match family.as_str() {
        "sdk" => {
            let config = build_sdk_run_config(
                &state,
                &runtime_strategy,
                &parameters,
                cost_profile.as_ref(),
            )?;
            fs::write(&config_snapshot, toml::to_string_pretty(&config)?)?;
            SdkRunConfig::load(&config_snapshot)
                .context("validate SDK run configuration snapshot")?;
            (
                JobExecutionPlan::Standard("run-strategy"),
                "tessera run-strategy",
            )
        }
        _ => bail!("this strategy is not runnable in this build; only SDK strategies can run"),
    };
    let source_files = load_strategy_source_bundle(&state, &runtime_strategy)?;
    let source_sha256 = runtime_strategy
        .source_sha256
        .clone()
        .unwrap_or_else(|| hash_source_files(&source_files));
    let source_snapshot = output_dir.join("source_snapshot");
    for file in &source_files {
        let target = source_snapshot.join(checked_workspace_relative(&file.path)?);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(target, &file.content)?;
    }
    let manifest = serde_json::json!({
        "run_id": job.run_id,
        "job_id": job.id,
        "strategy_id": job.strategy_id,
        "start_date": job.start_date,
        "end_date": job.end_date,
        "created_at": job.created_at,
        "parameters": parameters,
        "costs_enabled": job.costs_enabled,
        "cost_profile": cost_profile,
        "config_snapshot": relative_to_root(&state.root, &config_snapshot),
        "source_sha256": source_sha256,
        "source_files": source_files.iter().map(|file| file.path.as_str()).collect::<Vec<_>>(),
        "source_snapshot": relative_to_root(&state.root, &source_snapshot),
        "engine": engine_name
    });
    fs::write(
        output_dir.join("run_manifest.json"),
        serde_json::to_vec_pretty(&manifest)?,
    )?;
    let started_at = Utc::now().to_rfc3339();
    {
        let connection = state.database.lock().expect("database lock poisoned");
        connection.execute(
            "UPDATE jobs SET status='running', started_at=?2 WHERE id=?1",
            params![job_id, started_at],
        )?;
        connection.execute(
            "UPDATE runs SET status='Running' WHERE id=?1",
            [&job.run_id],
        )?;
    }

    let custom_engine: Option<String> = {
        let connection = state.database.lock().expect("database lock poisoned");
        connection.query_row(
            "SELECT engine_path FROM strategies WHERE id=?1",
            [&runtime_strategy.id],
            |row| row.get(0),
        )?
    };
    let engine = if let Some(relative) = custom_engine {
        state.root.join(checked_workspace_relative(&relative)?)
    } else if state.root.join("target/release/tessera").is_file() {
        state.root.join("target/release/tessera")
    } else {
        state.root.join("target/debug/tessera")
    };
    if !engine.is_file() {
        bail!("tessera engine is not built; run cargo build --release --bin tessera");
    }
    let mut outputs = Vec::new();
    match plan {
        JobExecutionPlan::Standard(command_name) => {
            // stderr streams straight into worker.log while the engine runs so progress
            // lines are visible before the job finishes; stdout is captured as before.
            let live_log = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&log_path)?;
            {
                use std::io::Write;
                writeln!(&live_log, "--- simulation stderr (live) ---")?;
            }
            let output = Command::new(&engine)
                .current_dir(&state.root)
                .arg(command_name)
                .arg("--config")
                .arg(&config_snapshot)
                .arg("--start")
                .arg(&job.start_date)
                .arg("--end")
                .arg(&job.end_date)
                .arg("--output-dir")
                .arg(&output_dir)
                .stdout(std::process::Stdio::piped())
                // tokio's `output()` would force stderr back to a pipe, so spawn explicitly.
                .stderr(std::process::Stdio::from(live_log))
                .spawn()?
                .wait_with_output()
                .await?;
            outputs.push(("simulation", output));
        }
    }
    let report_path = output_dir.join("report.html");
    let needs_standard_report = matches!(plan, JobExecutionPlan::Standard(_));
    if needs_standard_report && outputs.iter().all(|(_, output)| output.status.success()) {
        let report = Command::new(&engine)
            .current_dir(&state.root)
            .arg("report")
            .arg("--results-dir")
            .arg(&output_dir)
            .arg("--output")
            .arg(&report_path)
            .output()
            .await?;
        outputs.push(("report", report));
    }
    let succeeded = !outputs.is_empty()
        && outputs.iter().all(|(_, output)| output.status.success())
        && report_path.is_file();
    let exit_code = outputs
        .last()
        .map(|(_, output)| output.status.code().unwrap_or(-1))
        .unwrap_or(-1);
    let mut log = fs::read(&log_path).unwrap_or_default();
    log.push(b'\n');
    for (label, output) in &outputs {
        log.extend_from_slice(format!("--- {label} stdout ---\n").as_bytes());
        log.extend_from_slice(&output.stdout);
        log.extend_from_slice(format!("\n--- {label} stderr ---\n").as_bytes());
        log.extend_from_slice(&output.stderr);
        log.push(b'\n');
    }
    fs::write(&log_path, &log)?;
    let finished_at = Utc::now().to_rfc3339();
    let metrics_dir = report_path
        .parent()
        .filter(|_| succeeded)
        .map(Path::to_path_buf);
    let metrics = match metrics_dir {
        Some(dir) => tokio::task::spawn_blocking(move || compute_metrics_for_dir(&dir))
            .await
            .context("metrics task failed")?,
        None => None,
    };
    let metrics_json = serde_json::to_string(&metrics)?;
    let connection = state.database.lock().expect("database lock poisoned");
    if succeeded {
        connection.execute(
            "UPDATE jobs SET status='complete', finished_at=?2 WHERE id=?1",
            params![job_id, finished_at],
        )?;
        connection.execute(
            "UPDATE runs SET status='Complete', report_path=?2, exit_code=?3, metrics_json=?4 WHERE id=?1",
            params![
                job.run_id,
                relative_to_root(&state.root, &report_path),
                exit_code,
                metrics_json
            ],
        )?;
    } else {
        // Surface the engine's own reason (its last `Error:` line) instead of just the code.
        let reason = String::from_utf8_lossy(&log)
            .lines()
            .rev()
            .find(|line| line.starts_with("Error:"))
            .map(|line| line.trim_start_matches("Error:").trim().to_owned());
        let error = match reason {
            Some(reason) => format!("{reason} (exit code {exit_code}; see {})", job.log_path),
            None => format!("tessera exited with code {exit_code}; see {}", job.log_path),
        };
        connection.execute(
            "UPDATE jobs SET status='failed', finished_at=?2, error=?3 WHERE id=?1",
            params![job_id, finished_at, error],
        )?;
        connection.execute(
            "UPDATE runs SET status='Failed', exit_code=?2 WHERE id=?1",
            params![job.run_id, exit_code],
        )?;
    }
    Ok(())
}

fn mark_job_failed(state: &AppState, job_id: &str, error: &str) -> Result<()> {
    let connection = state.database.lock().expect("database lock poisoned");
    let run_id: String =
        connection.query_row("SELECT run_id FROM jobs WHERE id=?1", [job_id], |row| {
            row.get(0)
        })?;
    connection.execute(
        "UPDATE jobs SET status='failed', finished_at=?2, error=?3 WHERE id=?1",
        params![job_id, Utc::now().to_rfc3339(), error],
    )?;
    connection.execute("UPDATE runs SET status='Failed' WHERE id=?1", [run_id])?;
    Ok(())
}

fn active_worker_count(state: &AppState) -> usize {
    2usize.saturating_sub(state.workers.available_permits())
}

#[derive(Debug)]
struct ApiError(anyhow::Error);

impl<E> From<E> for ApiError
where
    E: Into<anyhow::Error>,
{
    fn from(error: E) -> Self {
        Self(error.into())
    }
}

impl axum::response::IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": format!("{:#}", self.0) })),
        )
            .into_response()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn parses_engine_progress_lines() {
        let progress =
            super::parse_progress_line("progress: replay 312/1926 2021-03-04 elapsed=17s")
                .expect("parsed");
        assert_eq!(progress.stage, "replay");
        assert_eq!((progress.done, progress.total), (312, 1926));
        assert_eq!(progress.label, "2021-03-04");
        assert_eq!(progress.elapsed_seconds, 17);
        assert!((progress.percent - 16.2).abs() < 0.1);
        let load =
            super::parse_progress_line("progress: load 17108/17993 symbols loaded elapsed=0s")
                .expect("parsed");
        assert_eq!(load.label, "symbols loaded");
        assert!(super::parse_progress_line("not progress").is_none());
    }

    use super::*;

    #[test]
    fn sweep_expansion_is_deterministic_and_axis_ordered() {
        let axes = vec![
            SweepAxis {
                parameter: "minimum_absolute_gap_z".to_owned(),
                values: vec![serde_json::json!(0.75), serde_json::json!(1.0)],
            },
            SweepAxis {
                parameter: "stop_loss_percent".to_owned(),
                values: vec![serde_json::json!(0.01), serde_json::json!(0.02)],
            },
        ];
        let combinations =
            expand_sweep_parameters(&serde_json::json!({}), &axes).expect("expand parameter grid");
        assert_eq!(combinations.len(), 4);
        assert_eq!(combinations[0]["minimum_absolute_gap_z"], 0.75);
        assert_eq!(combinations[0]["stop_loss_percent"], 0.01);
        assert_eq!(combinations[3]["minimum_absolute_gap_z"], 1.0);
        assert_eq!(combinations[3]["stop_loss_percent"], 0.02);
    }

    #[test]
    fn sweep_rejects_more_than_twenty_five_configurations() {
        let request = CreateSweepRequest {
            strategy_id: "iwm_mdy_gap_fade_v1".to_owned(),
            name: "oversized".to_owned(),
            start_date: "2020-01-01".to_owned(),
            end_date: "2023-12-31".to_owned(),
            base_parameters: serde_json::json!({}),
            axes: vec![
                SweepAxis {
                    parameter: "minimum_absolute_gap_z".to_owned(),
                    values: (1..=5)
                        .map(|value| serde_json::json!(value as f64 / 10.0))
                        .collect(),
                },
                SweepAxis {
                    parameter: "stop_loss_percent".to_owned(),
                    values: (1..=6)
                        .map(|value| serde_json::json!(value as f64 / 100.0))
                        .collect(),
                },
            ],
            costs_enabled: true,
            cost_profile_id: None,
        };
        assert!(validate_sweep_request(&request).is_err());
    }

    #[test]
    fn unconstrained_portfolio_accepts_two_full_capital_sleeves() {
        let request = CreatePortfolioRequest {
            name: "Full sleeves".to_owned(),
            initial_capital: 100_000.0,
            capital_mode: "unconstrained_overlays".to_owned(),
            components: vec![
                CreatePortfolioComponentRequest {
                    run_id: "one".to_owned(),
                    weight: 1.0,
                    capital_group: Some("overlay".to_owned()),
                },
                CreatePortfolioComponentRequest {
                    run_id: "two".to_owned(),
                    weight: 1.0,
                    capital_group: Some("overlay".to_owned()),
                },
            ],
        };
        validate_portfolio_request(&request).expect("validate full-sleeve portfolio");
    }

    #[test]
    fn sequential_portfolio_rejects_more_than_full_capital() {
        let request = CreatePortfolioRequest {
            name: "Invalid sequential".to_owned(),
            initial_capital: 100_000.0,
            capital_mode: "sequential_full_capital".to_owned(),
            components: vec![
                CreatePortfolioComponentRequest {
                    run_id: "one".to_owned(),
                    weight: 1.01,
                    capital_group: None,
                },
                CreatePortfolioComponentRequest {
                    run_id: "two".to_owned(),
                    weight: 1.0,
                    capital_group: None,
                },
            ],
        };
        assert!(validate_portfolio_request(&request).is_err());
    }

    #[test]
    fn strategy_source_hash_is_order_independent_and_content_sensitive() {
        let first = StrategySourceFile {
            path: "src/a.rs".to_owned(),
            content: "fn a() {}\n".to_owned(),
            editable: false,
        };
        let second = StrategySourceFile {
            path: "src/b.rs".to_owned(),
            content: "fn b() {}\n".to_owned(),
            editable: false,
        };
        assert_eq!(
            hash_source_files(&[first.clone(), second.clone()]),
            hash_source_files(&[second.clone(), first.clone()])
        );
        let mut changed = second;
        changed.content.push_str("// changed\n");
        assert_ne!(
            hash_source_files(&[first.clone(), changed]),
            hash_source_files(&[first, changed_source_file()])
        );
    }

    fn changed_source_file() -> StrategySourceFile {
        StrategySourceFile {
            path: "src/b.rs".to_owned(),
            content: "fn b() {}\n".to_owned(),
            editable: false,
        }
    }

    #[test]
    fn custom_strategy_ids_are_safe_workspace_slugs() {
        validate_strategy_slug("gap_fade_custom_v2").expect("valid strategy slug");
        assert!(validate_strategy_slug("Gap Fade v2").is_err());
        assert!(validate_strategy_slug("../gap_fade").is_err());
        assert!(validate_strategy_slug("2bad").is_err());
    }
}
