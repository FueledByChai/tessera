use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result, bail};
use axum::extract::{Path as AxumPath, Query, State};
use axum::http::{HeaderValue, Method, StatusCode};
use axum::response::{Html, IntoResponse};
use axum::routing::{delete, get, post, put};
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
use tessera::provider::budget::{CallBudget, Estimate};
use tessera::provider::eodhd::Eodhd;
use tessera::provider::jobs::eod::{
    self as eod_job, DEFAULT_MIN_BULK_ROWS, DatasetSymbol, EodJobInput, JobState, Skipped,
};
use tessera::provider::{Account, Listing, Provider, ProviderError};
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
    /// Inventory of configured data sources (sizes and coverage), refreshed every ten minutes.
    data_sources: Arc<Mutex<Option<(std::time::Instant, DataSourcesResponse)>>>,
    /// Where the EODHD adapter's calls go: the public API unless `TESSERA_EODHD_BASE_URL`
    /// points a scratch console at a stub (the service tests do the same in memory).
    eodhd_base_url: Arc<String>,
    /// The sources whose scan job is running (DS-05): a second scan on one is refused, and
    /// the card says `scanning` until the job has written its rows.
    scans: Arc<Mutex<std::collections::HashSet<String>>>,
    /// The download job running per source (DS-08, decision 0022: one job per source at a
    /// time), keyed by source id; a second is refused naming it, and its dataset shows
    /// `Updating` until it ends.
    dataset_jobs: Arc<Mutex<std::collections::HashMap<String, RunningJob>>>,
}

/// The job holding a source's per-source lock.
#[derive(Debug, Clone)]
struct RunningJob {
    id: String,
    dataset_id: String,
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
    /// Every run recorded against the strategy, whatever its status.
    run_count: usize,
    /// The newest completed run, so the catalog reads as a scoreboard (UI-06); `None` before
    /// the first completes.
    last_run: Option<StrategyLastRun>,
}

/// A strategy's newest completed run as the catalog shows it: when it ran and the headline
/// metrics cached on it (`None` when the run's report yielded none).
#[derive(Debug, Clone, Serialize)]
struct StrategyLastRun {
    run_id: String,
    created_at: String,
    metrics: Option<RunMetrics>,
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
    /// What loading dropped or skipped (`sanitation.json`; absent on runs that predate it).
    sanitation: Option<tessera::sdk::runner::RunSanitation>,
}

#[derive(Debug, Serialize)]
struct DashboardResponse {
    strategies: Vec<StrategyRecord>,
    recent_runs: Vec<RunRecord>,
    jobs: Vec<JobRecord>,
    production_strategies: usize,
    archived_strategies: usize,
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
    seed_feature_presets(&connection)?;

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
        data_sources: Arc::new(Mutex::new(None)),
        eodhd_base_url: Arc::new(
            std::env::var("TESSERA_EODHD_BASE_URL")
                .unwrap_or_else(|_| tessera::provider::eodhd::DEFAULT_BASE_URL.to_owned()),
        ),
        scans: Arc::new(Mutex::new(std::collections::HashSet::new())),
        dataset_jobs: Arc::new(Mutex::new(std::collections::HashMap::new())),
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

    // The console bundle is served from this process, so the browser talks to the API on the
    // same origin. CORS remains only for the Vite dev server (`npm run dev` on 5173).
    let cors = CorsLayer::new()
        .allow_origin([
            "http://127.0.0.1:5173".parse::<HeaderValue>()?,
            "http://localhost:5173".parse::<HeaderValue>()?,
        ])
        .allow_methods([Method::GET, Method::POST, Method::PUT, Method::DELETE])
        .allow_headers([axum::http::header::CONTENT_TYPE]);
    let web_dist = state.root.join("web").join("dist");
    let web_ready = web_dist.join("index.html").is_file();
    // Unknown paths (client-side views, deep links) fall through to index.html.
    let static_site = tower_http::services::ServeDir::new(&web_dist)
        .append_index_html_on_directories(true)
        .fallback(tower_http::services::ServeFile::new(
            web_dist.join("index.html"),
        ));

    let app = api_router()
        .fallback_service(static_site)
        .layer(cors)
        .with_state(state);

    // TESSERA_ADDR moves a scratch instance off the real console's port (scripts/scratch-console.sh).
    let address = std::env::var("TESSERA_ADDR").unwrap_or_else(|_| "127.0.0.1:8787".to_owned());
    let listener = tokio::net::TcpListener::bind(&address).await?;
    if web_ready {
        println!("Tessera console at http://{address}/ (API under /api)");
    } else {
        println!(
            "Tessera API listening on http://{address}; console bundle not built yet (run `npm run build` in web/)"
        );
    }
    axum::serve(listener, app).await?;
    Ok(())
}

/// The API routes, without the static site, so the service tests serve exactly what the
/// console talks to.
fn api_router() -> Router<AppState> {
    Router::new()
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
        .route("/api/studies", get(list_studies).post(create_study))
        .route("/api/studies/series", get(list_study_series))
        .route("/api/studies/{id}", get(study_detail))
        .route("/api/studies/{id}/export", get(study_export))
        .route(
            "/api/features",
            get(list_feature_presets).post(create_feature_preset),
        )
        .route("/api/features/{id}", delete(remove_feature_preset))
        .route("/api/features/{id}/promote", post(promote_feature_preset))
        .route("/api/lake/instruments", get(lake_instruments))
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
        .route("/api/data/sources", get(data_sources))
        .route("/api/instruments", get(search_instrument_catalog))
        .route(
            "/api/strategy-drafts/{id}/build",
            post(build_strategy_draft),
        )
        .route("/api/data/update-eod", post(start_eod_update))
        .route("/api/sources", get(list_sources).post(create_source))
        .route(
            "/api/sources/{id}",
            put(update_source).delete(delete_source),
        )
        .route("/api/sources/{id}/token", put(replace_token))
        .route("/api/sources/{id}/verify", post(verify_source))
        .route("/api/sources/{id}/availability", get(availability))
        .route(
            "/api/sources/{id}/availability/refresh",
            post(refresh_availability),
        )
        .route("/api/sources/{id}/datasets", post(create_dataset))
        .route("/api/sources/{id}/scan", post(scan_source))
        .route("/api/datasets/{id}", delete(delete_dataset))
        .route("/api/datasets/{id}/update", post(start_dataset_update))
        .route("/api/datasets/jobs/{id}", get(get_dataset_job))
        .route("/api/datasets/jobs/{id}/log", get(get_dataset_job_log))
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
         CREATE TABLE IF NOT EXISTS feature_presets (
             id TEXT PRIMARY KEY,
             name TEXT NOT NULL UNIQUE,
             expression TEXT NOT NULL,
             note TEXT NOT NULL DEFAULT '',
             accepted INTEGER NOT NULL DEFAULT 0,
             created_at TEXT NOT NULL,
             promoted_at TEXT
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
         CREATE TABLE IF NOT EXISTS studies (
             id TEXT PRIMARY KEY,
             name TEXT NOT NULL,
             status TEXT NOT NULL,
             start_date TEXT NOT NULL,
             end_date TEXT NOT NULL,
             config_json TEXT NOT NULL,
             created_at TEXT NOT NULL,
             finished_at TEXT,
             error TEXT,
             artifact_dir TEXT NOT NULL
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
         CREATE TABLE IF NOT EXISTS data_sources (
             id TEXT PRIMARY KEY,
             name TEXT NOT NULL,
             kind TEXT NOT NULL,
             root TEXT NOT NULL,
             catalog_dir TEXT NOT NULL,
             reserve_pct REAL NOT NULL DEFAULT 5,
             token_set_at TEXT,
             verified_at TEXT,
             verify_state TEXT NOT NULL DEFAULT 'unverified',
             verify_message TEXT,
             created_at TEXT NOT NULL
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
         ON strategy_validations(draft_id, created_at DESC);
         CREATE TABLE IF NOT EXISTS provider_exchanges (
             source_id TEXT NOT NULL REFERENCES data_sources(id) ON DELETE CASCADE,
             code TEXT NOT NULL,
             name TEXT NOT NULL,
             country TEXT NOT NULL,
             resolutions TEXT NOT NULL,
             fetched_at TEXT NOT NULL,
             PRIMARY KEY (source_id, code)
         );
         CREATE TABLE IF NOT EXISTS provider_listings (
             source_id TEXT NOT NULL REFERENCES data_sources(id) ON DELETE CASCADE,
             exchange TEXT NOT NULL,
             code TEXT NOT NULL,
             name TEXT NOT NULL,
             type TEXT NOT NULL,
             currency TEXT NOT NULL,
             delisted INTEGER NOT NULL DEFAULT 0,
             fetched_at TEXT NOT NULL
         );
         CREATE INDEX IF NOT EXISTS idx_provider_listings_exchange
         ON provider_listings(source_id, exchange, delisted);
         CREATE TABLE IF NOT EXISTS provider_refreshes (
             source_id TEXT PRIMARY KEY REFERENCES data_sources(id) ON DELETE CASCADE,
             attempted_at TEXT NOT NULL,
             error TEXT
         );
         CREATE TABLE IF NOT EXISTS datasets (
             id TEXT PRIMARY KEY,
             source_id TEXT NOT NULL REFERENCES data_sources(id) ON DELETE CASCADE,
             exchange TEXT NOT NULL,
             types_json TEXT NOT NULL,
             resolution TEXT NOT NULL,
             from_date TEXT NOT NULL,
             folder TEXT NOT NULL,
             include_delisted INTEGER NOT NULL DEFAULT 0,
             created_at TEXT NOT NULL
         );
         CREATE INDEX IF NOT EXISTS idx_datasets_source ON datasets(source_id, created_at);
         CREATE TABLE IF NOT EXISTS dataset_scans (
             dataset_id TEXT PRIMARY KEY REFERENCES datasets(id) ON DELETE CASCADE,
             scanned_at TEXT NOT NULL,
             listed INTEGER NOT NULL,
             on_disk INTEGER NOT NULL,
             latest_date TEXT,
             current_count INTEGER NOT NULL,
             bytes INTEGER NOT NULL,
             uncataloged_json TEXT NOT NULL,
             state TEXT NOT NULL,
             error TEXT
         );
         CREATE TABLE IF NOT EXISTS dataset_jobs (
             id TEXT PRIMARY KEY,
             dataset_id TEXT NOT NULL REFERENCES datasets(id) ON DELETE CASCADE,
             kind TEXT NOT NULL,
             state TEXT NOT NULL,
             percent INTEGER NOT NULL DEFAULT 0,
             created_at TEXT NOT NULL,
             started_at TEXT,
             finished_at TEXT,
             calls INTEGER NOT NULL DEFAULT 0,
             added INTEGER NOT NULL DEFAULT 0,
             updated INTEGER NOT NULL DEFAULT 0,
             skipped_json TEXT NOT NULL DEFAULT '[]',
             estimate_json TEXT,
             error TEXT,
             log_path TEXT NOT NULL
         );
         CREATE INDEX IF NOT EXISTS idx_dataset_jobs_dataset_created
         ON dataset_jobs(dataset_id, created_at DESC);",
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
    // DS-07: the usage the provider last reported for a source, and when it was asked.
    ensure_column(connection, "data_sources", "requests_today", "INTEGER")?;
    ensure_column(connection, "data_sources", "daily_limit", "INTEGER")?;
    ensure_column(connection, "data_sources", "resets_at", "TEXT")?;
    ensure_column(connection, "data_sources", "usage_checked_at", "TEXT")?;
    // DS-08: the row count a bulk day must reach for a dataset, and the listing's country
    // and venue, which the regenerated catalog.csv carries.
    ensure_column(
        connection,
        "datasets",
        "min_bulk_rows",
        &format!("INTEGER NOT NULL DEFAULT {DEFAULT_MIN_BULK_ROWS}"),
    )?;
    ensure_column(
        connection,
        "provider_listings",
        "country",
        "TEXT NOT NULL DEFAULT ''",
    )?;
    ensure_column(
        connection,
        "provider_listings",
        "venue",
        "TEXT NOT NULL DEFAULT ''",
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
    // by earlier builds are archived: not runnable, hidden from the catalog by default, but kept
    // so their runs, reports, and portfolios still resolve.
    connection.execute(
        "UPDATE strategies SET runnable = 0, status = 'Archived'
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

/// The vol feature set (WB-15): named expressions the feature library starts with, so a
/// realized-vol study is a click per feature. Each is inserted only when no preset of that
/// name exists, so an edit or a deletion by the owner survives every later start.
pub const SEEDED_FEATURE_PRESETS: [(&str, &str, &str); 5] = [
    ("rv 5s", "mid | rv 5s", "realized vol, last 5 s (bps)"),
    ("rv 30s", "mid | rv 30s", "realized vol, last 30 s (bps)"),
    ("rv 60s", "mid | rv 60s", "realized vol, last 60 s (bps)"),
    (
        "vol ratio 5s/60s",
        "mid | rv 5s | ratio_to rv 60s",
        "5 s vol over 60 s vol",
    ),
    (
        "vol of vol 60s",
        "mid | rv 5s | std 60s",
        "std of the 5 s vol over 60 s (bps)",
    ),
];

fn seed_feature_presets(connection: &Connection) -> Result<()> {
    for (name, expression, note) in SEEDED_FEATURE_PRESETS {
        let exists: bool = connection.query_row(
            "SELECT COUNT(*) FROM feature_presets WHERE name = ?1",
            [name],
            |row| row.get::<_, i64>(0).map(|n| n > 0),
        )?;
        if !exists {
            save_feature_preset(connection, name, expression, note, false)?;
        }
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
    connection.execute(
        "UPDATE dataset_jobs SET state='Failed', finished_at=?1,
         error='Local service restarted before this download job finished; every file it wrote is whole, and the next run continues from the files.'
         WHERE state IN ('Queued', 'Running')",
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
    lake_dir: Option<PathBuf>,
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
    /// Tick data in the parquet lake: second resolutions and book features available.
    #[serde(default)]
    tick: bool,
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
        lake_dir: data.lake_dir.clone(),
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
            tick: false,
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
            tick: false,
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
    if let Some(lake) = roots.lake_dir.as_deref() {
        match tessera::lake::discover(lake) {
            Ok(instruments) => {
                for instrument in instruments {
                    let symbol = format!("{}:{}", instrument.exchange, instrument.symbol);
                    records.push(InstrumentRecord {
                        daily: false,
                        five_minute: false,
                        one_minute: false,
                        tick: instrument.has_trades,
                        asset_class: "Crypto".to_owned(),
                        exchange: instrument.exchange.clone(),
                        code: instrument.symbol.clone(),
                        suffix: instrument.exchange.clone(),
                        name: format!(
                            "{} tick data {} to {} ({} days{})",
                            instrument.exchange,
                            instrument.first_date,
                            instrument.last_date,
                            instrument.days,
                            if instrument.has_book { ", L2 book" } else { "" }
                        ),
                        name_upper: format!(
                            "{} {} {}",
                            instrument.exchange, instrument.symbol, "TICK CRYPTO"
                        )
                        .to_ascii_uppercase(),
                        currency: String::new(),
                        status: "tick".to_owned(),
                        symbol,
                    });
                }
            }
            Err(error) => eprintln!("lake discovery failed: {error:#}"),
        }
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
    // Tick records can build any intraday bar from trades; only daily is off the table.
    match resolution.to_ascii_lowercase().as_str() {
        "daily" | "eod" | "d" | "1d" => record.daily,
        "5m" | "five_minute" => record.five_minute || record.tick,
        "1m" | "one_minute" => record.one_minute || record.tick,
        other if other.ends_with('s') => record.tick,
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
    "fractional_units",
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
    // Active plus delisted common stocks, when the catalog provides the combined list.
    let all_stocks_universe = data.catalog_dir.join("all_stocks.txt");
    let mut symbols = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut push = |symbol: String| {
        // Nasdaq test symbols (ZVZZT, ZWZZT, ZXZZT, ZJZZT, TESTA...) ship in the catalog files
        // and print at whatever the test harness sent that day.
        if is_test_symbol(&symbol) {
            return;
        }
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
                "all_stocks" | "stocks_with_delisted" => vec![all_stocks_universe.as_path()],
                "etfs" | "us_etfs" => vec![etf_universe.as_path()],
                "all" | "stocks_and_etfs" => {
                    vec![stock_universe.as_path(), etf_universe.as_path()]
                }
                other => bail!(
                    "unknown universe {other:?}; use universe:stocks, universe:all_stocks (active plus delisted), universe:etfs, or universe:all"
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

fn is_test_symbol(symbol: &str) -> bool {
    let base = symbol.strip_suffix(".US").unwrap_or(symbol);
    (base.len() == 5 && base.starts_with('Z') && base.ends_with("ZZT")) || base.starts_with("TEST")
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
    // The manifest's required symbols (a hedge ETF, say) join the plan even when the form
    // listed only a universe; the runner repeats this for frozen configs run from the CLI.
    let symbols = manifest
        .with_required_symbols(&symbols)
        .iter()
        .map(|symbol| normalize_sdk_symbol(symbol))
        .collect::<Result<Vec<_>>>()?;
    {
        let lake_symbols = symbols
            .iter()
            .filter(|symbol| tessera::lake::is_lake_symbol(symbol))
            .count();
        if lake_symbols > 0 {
            anyhow::ensure!(
                state.local.data.lake_dir.is_some(),
                "EXCHANGE:SYMBOL instruments need lake_dir in local.toml"
            );
            anyhow::ensure!(
                lake_symbols == symbols.len(),
                "mix of tick-lake instruments (EXCHANGE:SYMBOL) and CSV symbols in one run is not supported"
            );
        }
    }
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
            sanitize_prices: true,
            lake_dir: state.local.data.lake_dir.clone(),
        },
        sizing: SdkSizingConfig {
            initial_capital,
            position_percent,
            min_price,
            fractional_units: object
                .get("fractional_units")
                .and_then(serde_json::Value::as_bool),
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
            if !manifest.required_symbols.is_empty() {
                rules.push(format!(
                    "Every run also loads {}, whether or not the symbol list includes it.",
                    manifest.required_symbols.join(", ")
                ));
            }
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
    let sanitation = fs::read_to_string(artifact_dir.join("sanitation.json"))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok());
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
        sanitation,
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

// ---------------------------------------------------------------------------
// Data sources inventory: what the console is configured to read, with coverage.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
struct FeedInventory {
    feed: String,
    path: String,
    exists: bool,
    files: usize,
    bytes: u64,
    first_date: Option<String>,
    last_date: Option<String>,
    note: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct CatalogInventory {
    path: String,
    exists: bool,
    catalog_rows: usize,
    stocks: usize,
    etfs: usize,
    extra_lists: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
struct CsvLibraryInventory {
    provider: String,
    calendar_symbol: String,
    feeds: Vec<FeedInventory>,
    catalog: CatalogInventory,
    freshness_file: Option<String>,
    freshness: Option<serde_json::Value>,
    update_command: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct LakeInventory {
    path: String,
    exists: bool,
    feeds: Vec<FeedInventory>,
    instruments: Vec<tessera::lake::LakeInstrument>,
    total_bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
struct ConfigInventory {
    local_toml: String,
    local_toml_exists: bool,
    bundled_example: bool,
    env_overrides: Vec<String>,
    memory_budget_gb: f64,
}

#[derive(Debug, Clone, Serialize)]
struct DataSourcesResponse {
    generated_at: String,
    config: ConfigInventory,
    csv_library: CsvLibraryInventory,
    lake: Option<LakeInventory>,
}

fn dir_stats(dir: &Path, extension: Option<&str>) -> (usize, u64) {
    let mut files = 0usize;
    let mut bytes = 0u64;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        let Ok(entries) = fs::read_dir(&current) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if extension.is_none_or(|ext| path.extension().is_some_and(|e| e == ext)) {
                files += 1;
                bytes += entry.metadata().map(|m| m.len()).unwrap_or(0);
            }
        }
    }
    (files, bytes)
}

fn csv_row_date(line: &str) -> Option<String> {
    let first = line.split(',').next()?.trim();
    if NaiveDate::parse_from_str(first, "%Y-%m-%d").is_ok() {
        return Some(first.to_owned());
    }
    let epoch: i64 = first.parse().ok()?;
    Some(
        chrono::DateTime::from_timestamp(epoch, 0)?
            .date_naive()
            .to_string(),
    )
}

fn first_csv_date(path: &Path) -> Option<String> {
    use std::io::{BufRead, BufReader};
    let file = fs::File::open(path).ok()?;
    let mut lines = BufReader::new(file).lines();
    lines.next();
    csv_row_date(&lines.next()?.ok()?)
}

fn last_csv_row_date(path: &Path) -> Option<String> {
    last_csv_row_field(path).and_then(|field| csv_row_date(&field))
}

fn count_lines(path: &Path) -> usize {
    fs::read_to_string(path)
        .map(|text| {
            text.lines()
                .filter(|line| !line.trim().is_empty() && !line.starts_with('#'))
                .count()
        })
        .unwrap_or(0)
}

fn build_data_sources(state: &AppState) -> Result<DataSourcesResponse> {
    let data = &state.local.data;
    let calendar = format!("{}.csv", data.calendar_symbol);
    let mut feeds = Vec::new();
    for (feed, dir) in [
        ("daily", &data.daily_dir),
        ("5m", &data.five_minute_dir),
        ("1m", &data.one_minute_dir),
    ] {
        let exists = dir.is_dir();
        let (files, bytes) = if exists {
            dir_stats(dir, Some("csv"))
        } else {
            (0, 0)
        };
        let sample = dir.join(&calendar);
        let (first_date, last_date, note) = if sample.is_file() {
            (
                first_csv_date(&sample),
                last_csv_row_date(&sample),
                Some(format!("coverage from {calendar}")),
            )
        } else {
            (
                None,
                None,
                exists.then(|| format!("{calendar} not present; coverage unknown")),
            )
        };
        feeds.push(FeedInventory {
            feed: feed.to_owned(),
            path: dir.display().to_string(),
            exists,
            files,
            bytes,
            first_date,
            last_date,
            note,
        });
    }
    let catalog_dir = &data.catalog_dir;
    let mut extra_lists = Vec::new();
    if let Ok(entries) = fs::read_dir(catalog_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.ends_with(".txt") && name != "stocks.txt" && name != "etfs.txt" {
                extra_lists.push(format!("{name} ({})", count_lines(&entry.path())));
            } else if entry.path().is_dir() {
                extra_lists.push(format!("{name}/"));
            }
        }
    }
    extra_lists.sort();
    let catalog = CatalogInventory {
        path: catalog_dir.display().to_string(),
        exists: catalog_dir.is_dir(),
        catalog_rows: count_lines(&catalog_dir.join("catalog.csv")).saturating_sub(1),
        stocks: count_lines(&data.stock_universe()),
        etfs: count_lines(&data.etf_universe()),
        extra_lists,
    };
    let freshness = data
        .freshness_file
        .as_ref()
        .and_then(|path| fs::read_to_string(path).ok())
        .and_then(|text| serde_json::from_str(&text).ok());
    let csv_library = CsvLibraryInventory {
        provider: data.provider.clone(),
        calendar_symbol: data.calendar_symbol.clone(),
        feeds,
        catalog,
        freshness_file: data
            .freshness_file
            .as_ref()
            .map(|p| p.display().to_string()),
        freshness,
        update_command: data.update_command.clone(),
    };
    let lake = data.lake_dir.as_ref().map(|root| {
        let mut feeds = Vec::new();
        let mut total_bytes = 0u64;
        for feed in [
            "trades",
            "book_snapshots",
            "book_events",
            "funding",
            "open_interest",
        ] {
            let dir = root.join(feed);
            let exists = dir.is_dir();
            let (files, bytes) = if exists {
                dir_stats(&dir, Some("parquet"))
            } else {
                (0, 0)
            };
            total_bytes += bytes;
            let mut dates: Vec<String> = Vec::new();
            let mut symbols = 0usize;
            if let Ok(exchanges) = fs::read_dir(&dir) {
                for exchange in exchanges.flatten() {
                    if let Ok(syms) = fs::read_dir(exchange.path()) {
                        for sym in syms.flatten() {
                            symbols += 1;
                            if let Ok(days) = fs::read_dir(sym.path()) {
                                for day in days.flatten() {
                                    let name = day.file_name().to_string_lossy().to_string();
                                    if let Some(date) = name.strip_prefix("date=") {
                                        dates.push(date.to_owned());
                                    }
                                }
                            }
                        }
                    }
                }
            }
            dates.sort();
            feeds.push(FeedInventory {
                feed: feed.to_owned(),
                path: dir.display().to_string(),
                exists,
                files,
                bytes,
                first_date: dates.first().cloned(),
                last_date: dates.last().cloned(),
                note: exists.then(|| format!("{symbols} exchange/symbol partitions")),
            });
        }
        LakeInventory {
            path: root.display().to_string(),
            exists: root.is_dir(),
            instruments: tessera::lake::discover(root).unwrap_or_default(),
            feeds,
            total_bytes,
        }
    });
    let local_toml = state.root.join(tessera::local_config::LOCAL_FILE);
    let env_overrides = [
        "TESSERA_DATA_ROOT",
        "TESSERA_ENGINE",
        "TESSERA_STRATEGY_DIRS",
        "TESSERA_MEMORY_BUDGET_GB",
    ]
    .into_iter()
    .filter(|name| std::env::var_os(name).is_some())
    .map(str::to_owned)
    .collect::<Vec<_>>();
    let memory_budget_gb = std::env::var("TESSERA_MEMORY_BUDGET_GB")
        .ok()
        .and_then(|v| v.parse::<f64>().ok())
        .unwrap_or_else(|| {
            std::process::Command::new("sysctl")
                .args(["-n", "hw.memsize"])
                .output()
                .ok()
                .and_then(|o| {
                    String::from_utf8_lossy(&o.stdout)
                        .trim()
                        .parse::<f64>()
                        .ok()
                })
                .map_or(8.0, |bytes| bytes * 0.5 / 1e9)
        });
    Ok(DataSourcesResponse {
        generated_at: Utc::now().to_rfc3339(),
        config: ConfigInventory {
            local_toml: local_toml.display().to_string(),
            local_toml_exists: local_toml.is_file(),
            bundled_example: data.provider == "bundled-example",
            env_overrides,
            memory_budget_gb,
        },
        csv_library,
        lake,
    })
}

async fn data_sources(
    State(state): State<AppState>,
    Query(query): Query<std::collections::HashMap<String, String>>,
) -> Result<Json<DataSourcesResponse>, ApiError> {
    let refresh = query
        .get("refresh")
        .is_some_and(|v| v == "1" || v == "true");
    if !refresh {
        let cache = state
            .data_sources
            .lock()
            .expect("data sources lock poisoned");
        if let Some((built, response)) = cache.as_ref() {
            if built.elapsed() < std::time::Duration::from_secs(600) {
                return Ok(Json(response.clone()));
            }
        }
    }
    let worker = state.clone();
    let response = tokio::task::spawn_blocking(move || build_data_sources(&worker))
        .await
        .context("data sources task failed")??;
    *state
        .data_sources
        .lock()
        .expect("data sources lock poisoned") = Some((std::time::Instant::now(), response.clone()));
    Ok(Json(response))
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

// ---------------------------------------------------------------------------
// Registered data sources (DS-03, decisions 0020 and 0021): a row per provider account in
// the catalog, its token in a 0600 file under data/ui/secrets/ that no response, log line,
// or error message names.
// ---------------------------------------------------------------------------

/// The provider kinds compiled in, as `data_sources.kind` names them; the console's Add
/// source form offers exactly these.
const PROVIDER_KINDS: &[&str] = &["eodhd"];

/// Default share of the daily limit kept back from jobs (decision 0022).
const DEFAULT_RESERVE_PCT: f64 = tessera::provider::budget::DEFAULT_RESERVE_PCT;

/// How long a source's usage figures stand before `GET /api/sources` asks the provider
/// again (DS-07); `?refresh=1`, a verify, and a job's end ask regardless.
const USAGE_CACHE: chrono::Duration = chrono::Duration::seconds(60);

/// A source's connection state, as the card shows it.
const VERIFY_CONNECTED: &str = "connected";
const VERIFY_REJECTED: &str = "credentials_rejected";
const VERIFY_UNREACHABLE: &str = "unreachable";

/// Where a source's token file lives, relative to the service root.
const SECRETS_DIR: &str = "data/ui/secrets";

/// The adapters compiled in, keyed by a source's `kind`. The `Provider` trait's futures are
/// return-position impl-trait, so it is not object-safe; this enum holds each adapter
/// concretely and a new provider adds a variant.
enum SourceAdapter {
    Eodhd(Eodhd),
}

impl SourceAdapter {
    fn new(state: &AppState, kind: &str, token: &str) -> Result<Self> {
        match kind {
            "eodhd" => Ok(SourceAdapter::Eodhd(Eodhd::new(
                &state.eodhd_base_url,
                token,
            ))),
            other => bail!(
                "unknown provider kind {other:?}; compiled in: {}",
                PROVIDER_KINDS.join(", ")
            ),
        }
    }

    /// The adapter for a registered source, built from the token on file; 409 when no token
    /// file exists (a restored catalog), as `verify` answers.
    fn from_file(state: &AppState, row: &SourceRow) -> Result<Self, ApiError> {
        let token = read_token(state, &row.id)
            .map_err(|e| api_error(StatusCode::CONFLICT, e.to_string()))?;
        Ok(SourceAdapter::new(state, &row.kind, &token)?)
    }
}

/// The enum is itself a `Provider`, so the jobs in the library crate run over it as they
/// run over any adapter.
impl Provider for SourceAdapter {
    async fn verify(&self, token: &str) -> Result<tessera::provider::Account, ProviderError> {
        match self {
            SourceAdapter::Eodhd(eodhd) => eodhd.verify(token).await,
        }
    }

    async fn exchanges(&self) -> Result<Vec<tessera::provider::Exchange>, ProviderError> {
        match self {
            SourceAdapter::Eodhd(eodhd) => eodhd.exchanges().await,
        }
    }

    async fn symbols(
        &self,
        exchange: &str,
        delisted: bool,
    ) -> Result<Vec<tessera::provider::Listing>, ProviderError> {
        match self {
            SourceAdapter::Eodhd(eodhd) => eodhd.symbols(exchange, delisted).await,
        }
    }

    async fn bulk_eod(
        &self,
        exchange: &str,
        date: NaiveDate,
    ) -> Result<Vec<tessera::provider::BulkBar>, ProviderError> {
        match self {
            SourceAdapter::Eodhd(eodhd) => eodhd.bulk_eod(exchange, date).await,
        }
    }

    async fn eod_history(
        &self,
        symbol: &str,
        from: NaiveDate,
    ) -> Result<Vec<tessera::provider::Bar>, ProviderError> {
        match self {
            SourceAdapter::Eodhd(eodhd) => eodhd.eod_history(symbol, from).await,
        }
    }

    async fn splits(
        &self,
        exchange: &str,
        date: NaiveDate,
    ) -> Result<Vec<tessera::provider::Split>, ProviderError> {
        match self {
            SourceAdapter::Eodhd(eodhd) => eodhd.splits(exchange, date).await,
        }
    }
}

/// The root volume's figures from statvfs, in bytes.
#[derive(Debug, Clone, Serialize)]
struct VolumeFigures {
    total_bytes: u64,
    used_bytes: u64,
    free_bytes: u64,
}

/// The usage the provider last reported for a source (DS-07): the card's credits line and
/// the numbers a job's budget starts from. `None` until the provider has answered once.
#[derive(Debug, Clone, Serialize)]
struct SourceUsage {
    requests_today: u64,
    daily_limit: u64,
    /// When the counter resets, RFC 3339 in UTC (EODHD: 00:00 UTC after the counted day).
    resets_at: String,
    /// When the provider reported these figures. A later failed check leaves them and this
    /// time in place: the card shows the last value with its time.
    checked_at: String,
    /// The reserve in calls: `reserve_pct` of the limit, rounded up.
    reserve_calls: u64,
    /// What a job may still spend: the limit less today's requests and the reserve.
    available_calls: u64,
}

/// A source card: everything the console shows. The token and its file's path are not here.
#[derive(Debug, Clone, Serialize)]
struct SourceCard {
    id: String,
    name: String,
    kind: String,
    root: String,
    catalog_dir: String,
    reserve_pct: f64,
    usage: Option<SourceUsage>,
    root_exists: bool,
    volume: Option<VolumeFigures>,
    /// Whether a token file exists for the source (a restored catalog may have the row and
    /// not the file; the token then has to be entered again).
    token_set: bool,
    token_set_at: Option<String>,
    verified_at: Option<String>,
    verify_state: String,
    verify_message: Option<String>,
    created_at: String,
    /// The datasets registered against the source, each with its last scan (DS-05).
    datasets: Vec<DatasetRow>,
    /// Files under the root that no dataset claims, per folder, as the last scan found them.
    uncataloged: Vec<UncatalogedFolder>,
    /// When the last scan ran; `None` before one has.
    scanned_at: Option<String>,
    /// Whether a scan of the source is running now.
    scanning: bool,
}

#[derive(Debug, Clone, Serialize)]
struct SourcesResponse {
    kinds: Vec<&'static str>,
    sources: Vec<SourceCard>,
}

#[derive(Debug, Deserialize)]
struct CreateSourceRequest {
    kind: String,
    name: String,
    root: String,
    catalog_dir: String,
    token: String,
    #[serde(default = "default_reserve_pct")]
    reserve_pct: f64,
}

fn default_reserve_pct() -> f64 {
    DEFAULT_RESERVE_PCT
}

#[derive(Debug, Deserialize)]
struct TokenRequest {
    token: String,
}

/// `PUT /api/sources/{id}`: the settings a card edits in place (the reserve; the token has
/// its own endpoint, the name and folders are fixed at registration).
#[derive(Debug, Deserialize)]
struct UpdateSourceRequest {
    reserve_pct: f64,
}

struct SourceRow {
    id: String,
    name: String,
    kind: String,
    root: String,
    catalog_dir: String,
    reserve_pct: f64,
    token_set_at: Option<String>,
    verified_at: Option<String>,
    verify_state: String,
    verify_message: Option<String>,
    created_at: String,
    requests_today: Option<u64>,
    daily_limit: Option<u64>,
    resets_at: Option<String>,
    usage_checked_at: Option<String>,
}

const SOURCE_COLUMNS: &str = "id, name, kind, root, catalog_dir, reserve_pct, token_set_at, \
     verified_at, verify_state, verify_message, created_at, requests_today, daily_limit, \
     resets_at, usage_checked_at";

fn map_source(row: &rusqlite::Row<'_>) -> rusqlite::Result<SourceRow> {
    Ok(SourceRow {
        id: row.get(0)?,
        name: row.get(1)?,
        kind: row.get(2)?,
        root: row.get(3)?,
        catalog_dir: row.get(4)?,
        reserve_pct: row.get(5)?,
        token_set_at: row.get(6)?,
        verified_at: row.get(7)?,
        verify_state: row.get(8)?,
        verify_message: row.get(9)?,
        created_at: row.get(10)?,
        requests_today: row.get(11)?,
        daily_limit: row.get(12)?,
        resets_at: row.get(13)?,
        usage_checked_at: row.get(14)?,
    })
}

/// The budget a job on this source starts from: the provider's last usage report against
/// the source's reserve. `None` until the provider has reported once, and a job must not
/// start on none (decision 0022: jobs are governed by the usage the provider reports).
fn budget_of(row: &SourceRow) -> Option<CallBudget> {
    Some(CallBudget::with_reserve_pct(
        row.daily_limit?,
        row.requests_today?,
        row.reserve_pct,
    ))
}

/// The card's usage record for a row, or `None` before the first report.
fn usage_of(row: &SourceRow) -> Option<SourceUsage> {
    let budget = budget_of(row)?;
    Some(SourceUsage {
        requests_today: budget.used,
        daily_limit: budget.limit,
        resets_at: row.resets_at.clone()?,
        checked_at: row.usage_checked_at.clone()?,
        reserve_calls: budget.reserve,
        available_calls: budget.available(),
    })
}

/// Whether a source was checked with the provider within the usage cache window, so a
/// listing may show what it has instead of asking again.
fn checked_recently(verified_at: Option<&str>, now: DateTime<Utc>) -> bool {
    verified_at
        .and_then(|text| DateTime::parse_from_rfc3339(text).ok())
        .is_some_and(|at| {
            let at = at.with_timezone(&Utc);
            at <= now && now - at < USAGE_CACHE
        })
}

fn load_source_rows(state: &AppState) -> Result<Vec<SourceRow>> {
    let connection = state.database.lock().expect("database lock poisoned");
    let mut statement = connection.prepare(&format!(
        "SELECT {SOURCE_COLUMNS} FROM data_sources ORDER BY created_at, id"
    ))?;
    let rows = statement
        .query_map([], map_source)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

fn load_source_row(state: &AppState, id: &str) -> Result<Option<SourceRow>> {
    let connection = state.database.lock().expect("database lock poisoned");
    Ok(connection
        .query_row(
            &format!("SELECT {SOURCE_COLUMNS} FROM data_sources WHERE id = ?1"),
            [id],
            map_source,
        )
        .optional()?)
}

/// The card for a row: the root's presence and volume figures, and whether its token file
/// exists, read at the time of the call; the datasets and the last scan from the catalog.
fn card_of(state: &AppState, row: SourceRow) -> Result<SourceCard> {
    let root = Path::new(&row.root);
    let root_exists = root.is_dir();
    let volume = if root_exists {
        volume_figures(root)
    } else {
        None
    };
    let token_set = token_path(state, &row.id).is_file();
    let usage = usage_of(&row);
    let (datasets, (scanned_at, uncataloged)) = {
        let connection = state.database.lock().expect("database lock poisoned");
        (
            load_datasets(&connection, &row.id)?,
            last_scan_of(&connection, &row.id)?,
        )
    };
    let scanning = state
        .scans
        .lock()
        .expect("scan set poisoned")
        .contains(&row.id);
    Ok(SourceCard {
        id: row.id,
        name: row.name,
        kind: row.kind,
        root: row.root,
        catalog_dir: row.catalog_dir,
        reserve_pct: row.reserve_pct,
        usage,
        root_exists,
        volume,
        token_set,
        token_set_at: row.token_set_at,
        verified_at: row.verified_at,
        verify_state: row.verify_state,
        verify_message: row.verify_message,
        created_at: row.created_at,
        datasets,
        uncataloged,
        scanned_at,
        scanning,
    })
}

fn load_source_card(state: &AppState, id: &str) -> Result<SourceCard, ApiError> {
    let row = load_source_row(state, id)?
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, format!("no source {id:?}")))?;
    Ok(card_of(state, row)?)
}

/// statvfs on `path`: the volume's total size, what is used, and what is free to this
/// process. `None` when the path cannot be stat'ed (an unmounted root).
fn volume_figures(path: &Path) -> Option<VolumeFigures> {
    use std::os::unix::ffi::OsStrExt;
    // The field types differ by platform (u32 counts on macOS, u64 on Linux).
    fn wide<T: Into<u64>>(n: T) -> u64 {
        n.into()
    }
    let c_path = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut stat = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    // SAFETY: statvfs reads a NUL-terminated path and writes the whole struct on success.
    let rc = unsafe { libc::statvfs(c_path.as_ptr(), stat.as_mut_ptr()) };
    if rc != 0 {
        return None;
    }
    // SAFETY: a zero return means statvfs filled the struct.
    let stat = unsafe { stat.assume_init() };
    let fragment = wide(stat.f_frsize);
    let blocks = wide(stat.f_blocks);
    Some(VolumeFigures {
        total_bytes: blocks.saturating_mul(fragment),
        used_bytes: blocks
            .saturating_sub(wide(stat.f_bfree))
            .saturating_mul(fragment),
        free_bytes: wide(stat.f_bavail).saturating_mul(fragment),
    })
}

/// Whether any regular file lies under `root` (the dataset folders live under it). A
/// missing or unreadable root holds none; symlinks are not followed.
fn holds_files(root: &Path) -> bool {
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_file() {
                return true;
            }
            if kind.is_dir() {
                pending.push(entry.path());
            }
        }
    }
    false
}

fn secrets_dir(state: &AppState) -> PathBuf {
    state.root.join(SECRETS_DIR)
}

fn token_path(state: &AppState, id: &str) -> PathBuf {
    secrets_dir(state).join(format!("{id}.token"))
}

/// Writes `token` for source `id` to a file only the service's user can read (0600 in a
/// 0700 folder), through a part file renamed over any earlier token so a replacement is
/// atomic. The errors name neither the token nor the path.
fn write_token(state: &AppState, id: &str, token: &str) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
    let dir = secrets_dir(state);
    if let Some(parent) = dir.parent() {
        fs::create_dir_all(parent).context("create the UI state folder")?;
    }
    if !dir.is_dir() {
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&dir)
            .context("create the secrets folder")?;
    }
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))
        .context("restrict the secrets folder")?;
    let part = dir.join(format!("{id}.token.part"));
    {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&part)
            .context("create the token file")?;
        file.write_all(token.as_bytes())
            .context("write the token file")?;
        file.sync_all().context("flush the token file")?;
    }
    fs::set_permissions(&part, fs::Permissions::from_mode(0o600))
        .context("restrict the token file")?;
    fs::rename(&part, token_path(state, id)).context("place the token file")?;
    Ok(())
}

/// The token on file for source `id`; an error, never naming the path, when there is none.
fn read_token(state: &AppState, id: &str) -> Result<String> {
    let text = fs::read_to_string(token_path(state, id))
        .map_err(|_| anyhow::anyhow!("no token is set for source {id:?}; replace it"))?;
    Ok(text.trim().to_owned())
}

fn remove_token(state: &AppState, id: &str) -> Result<()> {
    match fs::remove_file(token_path(state, id)) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => bail!("could not remove the token file for source {id:?}"),
    }
}

/// The card state and message a verification outcome records.
fn verify_state_of(error: &ProviderError) -> (&'static str, String) {
    match error {
        ProviderError::CredentialsRejected(message) => (VERIFY_REJECTED, message.clone()),
        other => (VERIFY_UNREACHABLE, other.to_string()),
    }
}

/// The refusal for a token that could not be verified while registering or replacing:
/// 422 with the provider's message for a rejected token, 502 when the provider could not
/// be asked. Nothing is saved either way.
fn refused(error: ProviderError) -> ApiError {
    match error {
        ProviderError::CredentialsRejected(_) => {
            api_error(StatusCode::UNPROCESSABLE_ENTITY, error.to_string())
        }
        _ => api_error(StatusCode::BAD_GATEWAY, error.to_string()),
    }
}

fn validate_source_request(request: &CreateSourceRequest) -> Result<()> {
    if !PROVIDER_KINDS.contains(&request.kind.as_str()) {
        bail!(
            "unknown provider kind {:?}; compiled in: {}",
            request.kind,
            PROVIDER_KINDS.join(", ")
        );
    }
    if request.name.trim().is_empty() {
        bail!("name is required");
    }
    if request.name.chars().count() > 80 {
        bail!("name must be at most 80 characters");
    }
    for (label, value) in [
        ("root", &request.root),
        ("catalog folder", &request.catalog_dir),
    ] {
        if value.trim().is_empty() {
            bail!("{label} is required");
        }
        if !Path::new(value).is_absolute() {
            bail!("{label} must be an absolute path");
        }
    }
    if request.token.trim().is_empty() {
        bail!("token is required");
    }
    validate_reserve_pct(request.reserve_pct)
}

fn validate_reserve_pct(reserve_pct: f64) -> Result<()> {
    if !(0.0..=100.0).contains(&reserve_pct) || reserve_pct.is_nan() {
        bail!("reserve_pct must be between 0 and 100");
    }
    Ok(())
}

/// Records what the provider said when asked about the account behind a source's token:
/// the usage figures with their time on success, or the card state and message on failure,
/// with `verified_at` the time of the check either way (DS-03's verify and DS-07's usage
/// refresh are the same `/api/user` call).
fn record_check(
    state: &AppState,
    id: &str,
    outcome: &Result<Account, ProviderError>,
    now: DateTime<Utc>,
) -> Result<()> {
    let stamp = now.to_rfc3339();
    let connection = state.database.lock().expect("database lock poisoned");
    match outcome {
        Ok(account) => {
            connection.execute(
                "UPDATE data_sources
                 SET verified_at = ?2, verify_state = ?3, verify_message = NULL,
                     requests_today = ?4, daily_limit = ?5, resets_at = ?6,
                     usage_checked_at = ?2
                 WHERE id = ?1",
                params![
                    id,
                    stamp,
                    VERIFY_CONNECTED,
                    account.requests_today,
                    account.daily_limit,
                    account.resets_at.to_rfc3339(),
                ],
            )?;
        }
        Err(error) => {
            let (verify_state, message) = verify_state_of(error);
            connection.execute(
                "UPDATE data_sources SET verified_at = ?2, verify_state = ?3, verify_message = ?4
                 WHERE id = ?1",
                params![id, stamp, verify_state, message],
            )?;
        }
    }
    Ok(())
}

/// Asks the provider about the account behind source `id` and records the answer
/// (`record_check`). Unless `force`, a source checked within `USAGE_CACHE` is left as it is.
/// A source with no token on file is recorded unreachable with that message; the returned
/// error is only for a catalog failure.
async fn refresh_source_usage(state: &AppState, row: &SourceRow, force: bool) -> Result<()> {
    let now = Utc::now();
    if !force && checked_recently(row.verified_at.as_deref(), now) {
        return Ok(());
    }
    let outcome = match read_token(state, &row.id) {
        Ok(token) => match SourceAdapter::new(state, &row.kind, &token) {
            Ok(adapter) => adapter.verify(&token).await,
            Err(error) => Err(ProviderError::Unreachable(error.to_string())),
        },
        Err(error) => Err(ProviderError::Unreachable(error.to_string())),
    };
    record_check(state, &row.id, &outcome, now)
}

/// The cards, each with its usage refreshed from the provider unless checked within the
/// last minute; `?refresh=1` asks regardless. A provider that cannot be reached leaves the
/// last figures with their time on the card and its state says so.
async fn list_sources(
    State(state): State<AppState>,
    Query(query): Query<std::collections::HashMap<String, String>>,
) -> Result<Json<SourcesResponse>, ApiError> {
    let force = query
        .get("refresh")
        .is_some_and(|v| v == "1" || v == "true");
    let worker = state.clone();
    let rows = tokio::task::spawn_blocking(move || load_source_rows(&worker))
        .await
        .context("sources task failed")??;
    for row in &rows {
        refresh_source_usage(&state, row, force).await?;
    }
    let worker = state.clone();
    let sources = tokio::task::spawn_blocking(move || -> Result<Vec<SourceCard>> {
        let rows = load_source_rows(&worker)?;
        rows.into_iter().map(|row| card_of(&worker, row)).collect()
    })
    .await
    .context("sources task failed")??;
    Ok(Json(SourcesResponse {
        kinds: PROVIDER_KINDS.to_vec(),
        sources,
    }))
}

async fn create_source(
    State(state): State<AppState>,
    Json(request): Json<CreateSourceRequest>,
) -> Result<(StatusCode, Json<SourceCard>), ApiError> {
    validate_source_request(&request)?;
    let root = request.root.trim();
    if let Some(taken) = load_source_rows(&state)?
        .into_iter()
        .find(|row| Path::new(&row.root) == Path::new(root))
    {
        return Err(api_error(
            StatusCode::CONFLICT,
            format!(
                "source {:?} already covers {}; one source per root",
                taken.name, taken.root
            ),
        ));
    }
    let token = request.token.trim().to_owned();
    let adapter = SourceAdapter::new(&state, &request.kind, &token)?;
    let account = adapter.verify(&token).await.map_err(refused)?;
    let now = Utc::now();
    let id = format!("source-{}", now.format("%Y%m%dT%H%M%S%.6fZ"));
    let stamp = now.to_rfc3339();
    write_token(&state, &id, &token)?;
    let inserted = {
        let connection = state.database.lock().expect("database lock poisoned");
        connection.execute(
            "INSERT INTO data_sources
             (id, name, kind, root, catalog_dir, reserve_pct, token_set_at, verified_at,
              verify_state, verify_message, created_at, requests_today, daily_limit,
              resets_at, usage_checked_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7, ?8, NULL, ?7, ?9, ?10, ?11, ?7)",
            params![
                id,
                request.name.trim(),
                request.kind,
                request.root.trim(),
                request.catalog_dir.trim(),
                request.reserve_pct,
                stamp,
                VERIFY_CONNECTED,
                account.requests_today,
                account.daily_limit,
                account.resets_at.to_rfc3339(),
            ],
        )
    };
    if let Err(error) = inserted {
        let _ = remove_token(&state, &id);
        return Err(anyhow::Error::from(error)
            .context("record the source")
            .into());
    }
    let card = load_source_card(&state, &id)?;
    Ok((StatusCode::CREATED, Json(card)))
}

async fn replace_token(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
    Json(request): Json<TokenRequest>,
) -> Result<Json<SourceCard>, ApiError> {
    let row = load_source_row(&state, &id)?
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, format!("no source {id:?}")))?;
    let token = request.token.trim().to_owned();
    require_api(!token.is_empty(), "token is required")?;
    let adapter = SourceAdapter::new(&state, &row.kind, &token)?;
    let account = adapter.verify(&token).await.map_err(refused)?;
    write_token(&state, &id, &token)?;
    let now = Utc::now();
    {
        let connection = state.database.lock().expect("database lock poisoned");
        connection.execute(
            "UPDATE data_sources SET token_set_at = ?2 WHERE id = ?1",
            params![id, now.to_rfc3339()],
        )?;
    }
    record_check(&state, &id, &Ok(account), now)?;
    Ok(Json(load_source_card(&state, &id)?))
}

/// Sets the source's reserve: the share of the daily limit jobs leave untouched (decision
/// 0022). The card comes back with the reserve in calls recomputed against the last usage.
async fn update_source(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
    Json(request): Json<UpdateSourceRequest>,
) -> Result<Json<SourceCard>, ApiError> {
    load_source_row(&state, &id)?
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, format!("no source {id:?}")))?;
    validate_reserve_pct(request.reserve_pct)?;
    {
        let connection = state.database.lock().expect("database lock poisoned");
        connection.execute(
            "UPDATE data_sources SET reserve_pct = ?2 WHERE id = ?1",
            params![id, request.reserve_pct],
        )?;
    }
    Ok(Json(load_source_card(&state, &id)?))
}

/// Re-checks the token on file and records the outcome on the card, usage included; the
/// response is the card whatever the provider said, since the check itself succeeded.
async fn verify_source(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<SourceCard>, ApiError> {
    let row = load_source_row(&state, &id)?
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, format!("no source {id:?}")))?;
    read_token(&state, &id).map_err(|e| api_error(StatusCode::CONFLICT, e.to_string()))?;
    refresh_source_usage(&state, &row, true).await?;
    Ok(Json(load_source_card(&state, &id)?))
}

/// Removes the record and its token file. Refused while any file lies under the source's
/// dataset folders, or under the root while it has no datasets: the console never deletes
/// data files (decision 0022).
async fn delete_source(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Result<StatusCode, ApiError> {
    let row = load_source_row(&state, &id)?
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, format!("no source {id:?}")))?;
    if let Some(running) = running_job_on(&state, &id) {
        return Err(api_error(
            StatusCode::CONFLICT,
            format!(
                "job {} is running on source {:?}; wait for it to finish",
                running.id, row.name
            ),
        ));
    }
    let mut folders: Vec<PathBuf> = {
        let connection = state.database.lock().expect("database lock poisoned");
        load_datasets(&connection, &id)?
            .iter()
            .map(|dataset| PathBuf::from(&dataset.folder))
            .collect()
    };
    if folders.is_empty() {
        folders.push(PathBuf::from(&row.root));
    }
    let occupied = tokio::task::spawn_blocking(move || {
        folders
            .into_iter()
            .find(|folder| holds_files(folder))
            .map(|folder| folder.display().to_string())
    })
    .await
    .context("dataset scan failed")?;
    if let Some(folder) = occupied {
        return Err(api_error(
            StatusCode::CONFLICT,
            format!(
                "source {:?} still has files under {folder}; the console never deletes data files",
                row.name
            ),
        ));
    }
    {
        let connection = state.database.lock().expect("database lock poisoned");
        connection.execute("DELETE FROM data_sources WHERE id = ?1", [&id])?;
    }
    remove_token(&state, &id)?;
    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// The provider's availability (DS-04, decisions 0013 and 0020): a source's exchange list and
// the listings of its exchanges, cached in the catalog with the time fetched and served with
// per-type counts. A refresh that fails keeps the rows fetched before and records why as a
// note, so the console shows a stale table as a visible state, never an empty one.
// ---------------------------------------------------------------------------

/// What a refresh may ask for: one exchange's listing instead of the listings of every
/// exchange with a dataset, and whether to fetch the delisted listing as well (one extra
/// call per exchange, cached apart); left out, an exchange's delisted listing is fetched
/// when one of its datasets includes delisted symbols.
#[derive(Debug, Default, Deserialize)]
struct RefreshAvailabilityRequest {
    exchange: Option<String>,
    delisted: Option<bool>,
}

/// The listed instruments of one of the provider's types, named as the provider names it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct TypeCount {
    #[serde(rename = "type")]
    kind: String,
    count: u64,
}

/// One exchange of the cached table, with its active listing counted when that is cached.
#[derive(Debug, Clone, Serialize)]
struct AvailableExchange {
    code: String,
    name: String,
    country: String,
    resolutions: Vec<String>,
    fetched_at: String,
    /// When the exchange's listing was fetched; `None` until it has been.
    listings_fetched_at: Option<String>,
    listed: u64,
    types: Vec<TypeCount>,
    /// The delisted listing, when it has been fetched: its time and its count.
    delisted_fetched_at: Option<String>,
    delisted: u64,
}

/// A source's cached availability: what `GET /api/sources/{id}/availability` serves.
#[derive(Debug, Clone, Serialize)]
struct AvailabilityResponse {
    source_id: String,
    /// When the exchange list was fetched; `None` until a refresh has succeeded.
    fetched_at: Option<String>,
    /// When a refresh was last attempted, whether or not it succeeded.
    refreshed_at: Option<String>,
    /// Why the last refresh failed, when it did; the rows are then the ones fetched before.
    unreachable: Option<String>,
    exchanges: Vec<AvailableExchange>,
}

/// Replaces the source's exchange rows with `exchanges`, all stamped `fetched_at`.
fn store_exchanges(
    connection: &mut Connection,
    source_id: &str,
    exchanges: &[tessera::provider::Exchange],
    fetched_at: &str,
) -> Result<()> {
    let tx = connection.transaction()?;
    tx.execute(
        "DELETE FROM provider_exchanges WHERE source_id = ?1",
        [source_id],
    )?;
    {
        let mut insert = tx.prepare(
            "INSERT OR REPLACE INTO provider_exchanges
             (source_id, code, name, country, resolutions, fetched_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )?;
        for exchange in exchanges {
            insert.execute(params![
                source_id,
                exchange.code,
                exchange.name,
                exchange.country,
                serde_json::to_string(&exchange.resolutions)?,
                fetched_at
            ])?;
        }
    }
    tx.commit()?;
    Ok(())
}

/// Replaces the source's cached listing of `exchange` (the active or the delisted one) with
/// `listings`, all stamped `fetched_at`.
fn store_listings(
    connection: &mut Connection,
    source_id: &str,
    exchange: &str,
    delisted: bool,
    listings: &[tessera::provider::Listing],
    fetched_at: &str,
) -> Result<()> {
    let tx = connection.transaction()?;
    tx.execute(
        "DELETE FROM provider_listings
         WHERE source_id = ?1 AND exchange = ?2 AND delisted = ?3",
        params![source_id, exchange, delisted],
    )?;
    {
        let mut insert = tx.prepare(
            "INSERT INTO provider_listings
             (source_id, exchange, code, name, type, currency, delisted, fetched_at,
              country, venue)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        )?;
        for listing in listings {
            insert.execute(params![
                source_id,
                exchange,
                listing.code,
                listing.name,
                listing.kind,
                listing.currency,
                delisted,
                fetched_at,
                listing.country,
                listing.venue
            ])?;
        }
    }
    tx.commit()?;
    Ok(())
}

/// Records a refresh attempt: its time and, when it failed, why.
fn record_refresh(
    connection: &Connection,
    source_id: &str,
    attempted_at: &str,
    error: Option<&str>,
) -> Result<()> {
    connection.execute(
        "INSERT INTO provider_refreshes (source_id, attempted_at, error) VALUES (?1, ?2, ?3)
         ON CONFLICT(source_id) DO UPDATE
         SET attempted_at = excluded.attempted_at, error = excluded.error",
        params![source_id, attempted_at, error],
    )?;
    Ok(())
}

/// The exchanges with a dataset registered against the source, each with whether one of
/// its datasets includes delisted symbols (so the refresh fetches that listing too).
fn exchanges_with_datasets(
    connection: &Connection,
    source_id: &str,
) -> Result<Vec<(String, bool)>> {
    let mut statement = connection.prepare(
        "SELECT exchange, MAX(include_delisted) FROM datasets
         WHERE source_id = ?1 GROUP BY exchange ORDER BY exchange",
    )?;
    let codes = statement
        .query_map([source_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? != 0))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(codes)
}

/// The cached table for a source, each exchange's active listing counted by type.
fn load_availability(connection: &Connection, source_id: &str) -> Result<AvailabilityResponse> {
    let (refreshed_at, unreachable) = connection
        .query_row(
            "SELECT attempted_at, error FROM provider_refreshes WHERE source_id = ?1",
            [source_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
        )
        .optional()?
        .map_or((None, None), |(attempted, error)| (Some(attempted), error));

    // exchange -> (listing fetched at, listed, counts by type, largest type first)
    let mut listings: std::collections::HashMap<String, (Option<String>, u64, Vec<TypeCount>)> =
        std::collections::HashMap::new();
    // exchange -> (delisted listing fetched at, delisted)
    let mut delisted_listings: std::collections::HashMap<String, (String, u64)> =
        std::collections::HashMap::new();
    {
        let mut statement = connection.prepare(
            "SELECT exchange, delisted, type, COUNT(*), MIN(fetched_at) FROM provider_listings
             WHERE source_id = ?1
             GROUP BY exchange, delisted, type ORDER BY exchange, delisted, COUNT(*) DESC, type",
        )?;
        let rows = statement.query_map([source_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)? != 0,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, String>(4)?,
            ))
        })?;
        for row in rows {
            let (exchange, delisted, kind, count, fetched_at) = row?;
            let count = u64::try_from(count).unwrap_or(0);
            if delisted {
                let entry = delisted_listings
                    .entry(exchange)
                    .or_insert_with(|| (fetched_at, 0));
                entry.1 += count;
                continue;
            }
            let entry = listings
                .entry(exchange)
                .or_insert_with(|| (None, 0, Vec::new()));
            entry.0.get_or_insert(fetched_at);
            entry.1 += count;
            entry.2.push(TypeCount { kind, count });
        }
    }

    let mut statement = connection.prepare(
        "SELECT code, name, country, resolutions, fetched_at FROM provider_exchanges
         WHERE source_id = ?1 ORDER BY code",
    )?;
    let rows = statement
        .query_map([source_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut fetched_at: Option<String> = None;
    let exchanges = rows
        .into_iter()
        .map(|(code, name, country, resolutions, fetched)| {
            fetched_at.get_or_insert_with(|| fetched.clone());
            let (listings_fetched_at, listed, types) =
                listings.remove(&code).unwrap_or((None, 0, Vec::new()));
            let (delisted_fetched_at, delisted) = delisted_listings
                .remove(&code)
                .map_or((None, 0), |(at, count)| (Some(at), count));
            AvailableExchange {
                resolutions: serde_json::from_str(&resolutions).unwrap_or_default(),
                code,
                name,
                country,
                fetched_at: fetched,
                listings_fetched_at,
                listed,
                types,
                delisted_fetched_at,
                delisted,
            }
        })
        .collect();
    Ok(AvailabilityResponse {
        source_id: source_id.to_owned(),
        fetched_at,
        refreshed_at,
        unreachable,
        exchanges,
    })
}

async fn availability(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<AvailabilityResponse>, ApiError> {
    load_source_row(&state, &id)?
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, format!("no source {id:?}")))?;
    let worker = state.clone();
    let response = tokio::task::spawn_blocking(move || {
        let connection = worker.database.lock().expect("database lock poisoned");
        load_availability(&connection, &id)
    })
    .await
    .context("availability task failed")??;
    Ok(Json(response))
}

/// Fetches the exchange list and the active listings of every exchange with a dataset, or of
/// the one exchange the body names (`{"exchange": "US"}`; an empty body means every one),
/// plus the delisted listing where `"delisted": true` asks for it or, unasked, where a
/// dataset of the exchange includes delisted symbols, and caches them stamped with the
/// time. 200 with the table whatever the provider said: a call that fails leaves the rows
/// fetched before and shows as the `unreachable` note.
async fn refresh_availability(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
    body: String,
) -> Result<Json<AvailabilityResponse>, ApiError> {
    let row = load_source_row(&state, &id)?
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, format!("no source {id:?}")))?;
    let request: RefreshAvailabilityRequest = if body.trim().is_empty() {
        RefreshAvailabilityRequest::default()
    } else {
        serde_json::from_str(&body).context("read the refresh request")?
    };
    let adapter = SourceAdapter::from_file(&state, &row)?;
    let attempted_at = Utc::now().to_rfc3339();

    let exchanges = match adapter.exchanges().await {
        Ok(exchanges) => exchanges,
        Err(error) => {
            let connection = state.database.lock().expect("database lock poisoned");
            record_refresh(&connection, &id, &attempted_at, Some(&error.to_string()))?;
            return Ok(Json(load_availability(&connection, &id)?));
        }
    };
    let wanted: Vec<(String, bool)> = match request.exchange {
        Some(code) => {
            let code = code.trim().to_owned();
            require_api(
                exchanges.iter().any(|exchange| exchange.code == code),
                format!("{} lists no exchange {code:?}", row.name),
            )?;
            vec![(code, request.delisted.unwrap_or(false))]
        }
        None => {
            let connection = state.database.lock().expect("database lock poisoned");
            exchanges_with_datasets(&connection, &id)?
                .into_iter()
                .map(|(code, includes)| (code, request.delisted.unwrap_or(includes)))
                .collect()
        }
    };
    {
        let mut connection = state.database.lock().expect("database lock poisoned");
        store_exchanges(&mut connection, &id, &exchanges, &attempted_at)?;
    }

    let mut failure: Option<String> = None;
    'exchanges: for (code, with_delisted) in wanted {
        let lists: &[bool] = if with_delisted {
            &[false, true]
        } else {
            &[false]
        };
        for &delisted in lists {
            match adapter.symbols(&code, delisted).await {
                Ok(listings) => {
                    let worker = state.clone();
                    let source_id = id.clone();
                    let stamp = attempted_at.clone();
                    let exchange = code.clone();
                    tokio::task::spawn_blocking(move || {
                        let mut connection =
                            worker.database.lock().expect("database lock poisoned");
                        store_listings(
                            &mut connection,
                            &source_id,
                            &exchange,
                            delisted,
                            &listings,
                            &stamp,
                        )
                    })
                    .await
                    .context("listing store task failed")??;
                }
                Err(error) => {
                    failure = Some(if delisted {
                        format!("{code} (delisted): {error}")
                    } else {
                        format!("{code}: {error}")
                    });
                    break 'exchanges;
                }
            }
        }
    }
    let worker = state.clone();
    let response = tokio::task::spawn_blocking(move || {
        let connection = worker.database.lock().expect("database lock poisoned");
        record_refresh(&connection, &id, &attempted_at, failure.as_deref())?;
        load_availability(&connection, &id)
    })
    .await
    .context("availability task failed")??;
    Ok(Json(response))
}

// ---------------------------------------------------------------------------
// Datasets and the scan cache (DS-05, decisions 0012, 0013, 0021): what a source keeps
// current, where it is, and how complete it is. A dataset is one exchange, a set of the
// provider's types, a resolution, a from-date, and a folder; its figures (listed against on
// disk, the latest date, the count current through the latest expected session, bytes) come
// from a scan that runs in the background on request and writes `dataset_scans`, never from
// a page load. A scan that cannot judge a dataset keeps its previous figures and says why.
// ---------------------------------------------------------------------------

/// The share of the listed symbols a dataset must hold files for to count as complete;
/// under it the state is Partial (a listing always carries a few symbols with no history).
const COMPLETE_SHARE: f64 = 0.95;

/// The states a scan assigns, as `docs/DATA_SOURCES.md` defines them (BT-605).
const STATE_CURRENT: &str = "Current";
const STATE_UPDATING: &str = "Updating";
const STATE_STALE: &str = "Stale";
const STATE_PARTIAL: &str = "Partial";
const STATE_FAILED: &str = "Failed";
const STATE_UNKNOWN: &str = "Unknown";
const STATE_UNAVAILABLE: &str = "Unavailable";

/// Loose files directly under the root are counted under this folder name.
const ROOT_FOLDER: &str = ".";

#[derive(Debug, Deserialize)]
struct CreateDatasetRequest {
    exchange: String,
    types: Vec<String>,
    resolution: String,
    from_date: String,
    /// Absolute, or relative to the source's root; omitted, `<root>/eod` for daily bars and
    /// `<root>/<resolution>` otherwise.
    folder: Option<String>,
    /// Omitted, on for daily bars and off otherwise.
    include_delisted: Option<bool>,
    /// The row count a bulk day must reach before the EOD job accepts it (DS-08, decision
    /// 0022); omitted, 10,000, a US session's order of magnitude.
    min_bulk_rows: Option<u64>,
}

/// A dataset's figures from its last scan. The state lives on the row beside it.
#[derive(Debug, Clone, Serialize)]
struct DatasetScan {
    scanned_at: String,
    listed: u64,
    on_disk: u64,
    latest_date: Option<String>,
    current_count: u64,
    bytes: u64,
    /// Why the last scan could not judge the dataset; the figures are then the previous
    /// scan's.
    error: Option<String>,
}

/// A dataset as the card shows it: the registration and the last scan.
#[derive(Debug, Clone, Serialize)]
struct DatasetRow {
    id: String,
    source_id: String,
    exchange: String,
    types: Vec<String>,
    resolution: String,
    from_date: String,
    folder: String,
    include_delisted: bool,
    /// A bulk day under this many rows is refused by the EOD job.
    min_bulk_rows: u64,
    created_at: String,
    /// Current, Updating, Stale, Partial, Failed, Unknown, or Unavailable.
    state: String,
    scan: Option<DatasetScan>,
    /// The newest download job on the dataset (DS-08); `None` before one has been queued.
    last_job: Option<DatasetJobRecord>,
}

/// Files under the root that no dataset claims, counted per folder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct UncatalogedFolder {
    /// The folder's path relative to the root (`.` for files directly under it).
    folder: String,
    files: u64,
    bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
struct ScanAccepted {
    source_id: String,
    scanning: bool,
}

const DATASET_COLUMNS: &str = "d.id, d.source_id, d.exchange, d.types_json, d.resolution, \
     d.from_date, d.folder, d.include_delisted, d.created_at, s.scanned_at, s.listed, \
     s.on_disk, s.latest_date, s.current_count, s.bytes, s.state, s.error, d.min_bulk_rows";

fn map_dataset(row: &rusqlite::Row<'_>) -> rusqlite::Result<DatasetRow> {
    let types_json: String = row.get(3)?;
    let scanned_at: Option<String> = row.get(9)?;
    let scan = scanned_at
        .map(|scanned_at| {
            Ok::<_, rusqlite::Error>(DatasetScan {
                scanned_at,
                listed: row.get::<_, i64>(10)?.max(0) as u64,
                on_disk: row.get::<_, i64>(11)?.max(0) as u64,
                latest_date: row.get(12)?,
                current_count: row.get::<_, i64>(13)?.max(0) as u64,
                bytes: row.get::<_, i64>(14)?.max(0) as u64,
                error: row.get(16)?,
            })
        })
        .transpose()?;
    let state: Option<String> = row.get(15)?;
    Ok(DatasetRow {
        id: row.get(0)?,
        source_id: row.get(1)?,
        exchange: row.get(2)?,
        types: serde_json::from_str(&types_json).unwrap_or_default(),
        resolution: row.get(4)?,
        from_date: row.get(5)?,
        folder: row.get(6)?,
        include_delisted: row.get::<_, i64>(7)? != 0,
        min_bulk_rows: row.get::<_, i64>(17)?.max(0) as u64,
        created_at: row.get(8)?,
        state: state.unwrap_or_else(|| STATE_UNKNOWN.to_owned()),
        scan,
        last_job: None,
    })
}

/// Attaches each dataset's newest download job; a dataset whose job is queued or running
/// shows `Updating` whatever its last scan said (BT-605).
fn attach_last_jobs(connection: &Connection, datasets: &mut [DatasetRow]) -> Result<()> {
    for dataset in datasets {
        dataset.last_job = last_job_of(connection, &dataset.id)?;
        if dataset
            .last_job
            .as_ref()
            .is_some_and(|job| job.state == "Queued" || job.state == "Running")
        {
            dataset.state = STATE_UPDATING.to_owned();
        }
    }
    Ok(())
}

/// The source's datasets, oldest first, each with its last scan when one has run and its
/// newest job.
fn load_datasets(connection: &Connection, source_id: &str) -> Result<Vec<DatasetRow>> {
    let mut statement = connection.prepare(&format!(
        "SELECT {DATASET_COLUMNS} FROM datasets d
         LEFT JOIN dataset_scans s ON s.dataset_id = d.id
         WHERE d.source_id = ?1 ORDER BY d.created_at, d.id"
    ))?;
    let mut rows = statement
        .query_map([source_id], map_dataset)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    attach_last_jobs(connection, &mut rows)?;
    Ok(rows)
}

fn load_dataset(connection: &Connection, id: &str) -> Result<Option<DatasetRow>> {
    let row = connection
        .query_row(
            &format!(
                "SELECT {DATASET_COLUMNS} FROM datasets d
                 LEFT JOIN dataset_scans s ON s.dataset_id = d.id WHERE d.id = ?1"
            ),
            [id],
            map_dataset,
        )
        .optional()?;
    let mut rows: Vec<DatasetRow> = row.into_iter().collect();
    attach_last_jobs(connection, &mut rows)?;
    Ok(rows.pop())
}

/// The symbols a dataset covers: the codes of its types on its exchange in the cached
/// listing, the delisted ones included when the dataset includes them. Empty when the
/// listing is not cached.
fn listed_symbols(connection: &Connection, dataset: &DatasetRow) -> Result<Vec<String>> {
    if dataset.types.is_empty() {
        return Ok(Vec::new());
    }
    let placeholders = (0..dataset.types.len())
        .map(|i| format!("?{}", i + 4))
        .collect::<Vec<_>>()
        .join(", ");
    let mut statement = connection.prepare(&format!(
        "SELECT DISTINCT code FROM provider_listings
         WHERE source_id = ?1 AND exchange = ?2 AND (delisted = 0 OR ?3)
           AND type IN ({placeholders})
         ORDER BY code"
    ))?;
    let mut values: Vec<&dyn rusqlite::ToSql> = vec![
        &dataset.source_id,
        &dataset.exchange,
        &dataset.include_delisted,
    ];
    values.extend(dataset.types.iter().map(|t| t as &dyn rusqlite::ToSql));
    let codes = statement
        .query_map(values.as_slice(), |row| row.get(0))?
        .collect::<rusqlite::Result<Vec<String>>>()?;
    Ok(codes)
}

/// The file a listing's bars live in: `<code>.<exchange>.csv`, the provider's symbol form.
fn symbol_file_name(code: &str, exchange: &str) -> String {
    format!("{code}.{exchange}.csv")
}

/// The folder a dataset defaults to under the root: `eod` for daily bars, else the
/// resolution's own name (`5m`, `1m`, `1h`).
fn default_dataset_folder(resolution: &str) -> &str {
    if resolution == "daily" {
        "eod"
    } else {
        resolution
    }
}

/// The first field of a bar file's last non-empty row, read from its tail: a date for
/// daily bars, an epoch second for intraday ones.
fn last_csv_row_field(path: &Path) -> Option<String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let tail = len.min(4096);
    file.seek(SeekFrom::Start(len - tail)).ok()?;
    let mut buf = vec![0u8; tail as usize];
    file.read_exact(&mut buf).ok()?;
    let text = String::from_utf8_lossy(&buf);
    text.lines()
        .rev()
        .filter(|line| !line.trim().is_empty())
        .find_map(|line| {
            let first = line.split(',').next()?.trim();
            (!first.is_empty()).then(|| first.to_owned())
        })
}

/// The latest session a dataset is expected to reach (BT-605): the calendar symbol's last
/// date for daily bars; for intraday bars that session's close in New York, as the epoch
/// second a file's last bar must reach (the close less one bar's length).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExpectedSession {
    Date(NaiveDate),
    Close { session: NaiveDate, epoch: i64 },
}

impl ExpectedSession {
    fn for_resolution(session: NaiveDate, resolution: &str) -> Self {
        use chrono::TimeZone;
        let Some(bar_seconds) = bar_seconds(resolution) else {
            return ExpectedSession::Date(session);
        };
        let close = chrono_tz::America::New_York
            .from_local_datetime(&session.and_time(NaiveTime::from_hms_opt(16, 0, 0).unwrap()))
            .single()
            .map(|at| at.timestamp())
            .unwrap_or(0);
        ExpectedSession::Close {
            session,
            epoch: close - bar_seconds,
        }
    }

    /// Whether a file whose last row starts with `field` reaches the expected session.
    fn reached_by(&self, field: &str) -> bool {
        let date =
            csv_row_date(field).and_then(|text| NaiveDate::parse_from_str(&text, "%Y-%m-%d").ok());
        match self {
            ExpectedSession::Date(expected) => date.is_some_and(|d| d >= *expected),
            ExpectedSession::Close { session, epoch } => match field.parse::<i64>() {
                Ok(last) => last >= *epoch,
                Err(_) => date.is_some_and(|d| d >= *session),
            },
        }
    }
}

/// The length of one intraday bar in seconds (`5m`, `1m`, `1h`); `None` for daily bars or a
/// resolution not in that form.
fn bar_seconds(resolution: &str) -> Option<i64> {
    let (digits, unit) = resolution.split_at(resolution.len().checked_sub(1)?);
    let n: i64 = digits.parse().ok()?;
    match unit {
        "m" => Some(n * 60),
        "h" => Some(n * 3600),
        _ => None,
    }
}

/// The calendar symbol's daily file, looked for in the dataset's own folder, then the
/// source's daily dataset folders, then the library's `daily_dir`.
fn calendar_file(state: &AppState, folder: &Path, daily_folders: &[PathBuf]) -> Option<PathBuf> {
    let name = format!("{}.csv", state.local.data.calendar_symbol);
    std::iter::once(folder.to_path_buf())
        .chain(daily_folders.iter().cloned())
        .chain(std::iter::once(state.local.data.daily_dir.clone()))
        .map(|dir| dir.join(&name))
        .find(|path| path.is_file())
}

/// One dataset's figures from a walk of its folder.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ScanFigures {
    listed: u64,
    on_disk: u64,
    latest_date: Option<String>,
    current_count: u64,
    bytes: u64,
    state: &'static str,
}

/// Counts the files of `symbols` in `folder` (part files and anything else are not among
/// them), their bytes, the latest last date, and how many reach `expected`; the state
/// follows. An error is a folder that cannot be read.
fn scan_dataset(
    folder: &Path,
    exchange: &str,
    symbols: &[String],
    expected: Option<ExpectedSession>,
) -> Result<ScanFigures> {
    let wanted: std::collections::HashSet<String> = symbols
        .iter()
        .map(|code| symbol_file_name(code, exchange))
        .collect();
    let entries = fs::read_dir(folder)
        .with_context(|| format!("read the dataset folder {}", folder.display()))?;
    let mut on_disk = 0u64;
    let mut bytes = 0u64;
    let mut current_count = 0u64;
    let mut latest_date: Option<String> = None;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if !wanted.contains(name) {
            continue;
        }
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if !kind.is_file() {
            continue;
        }
        on_disk += 1;
        bytes += entry.metadata().map(|m| m.len()).unwrap_or(0);
        if let Some(field) = last_csv_row_field(&entry.path()) {
            if let Some(date) = csv_row_date(&field)
                && latest_date
                    .as_deref()
                    .is_none_or(|latest| date.as_str() > latest)
            {
                latest_date = Some(date);
            }
            if expected.is_some_and(|expected| expected.reached_by(&field)) {
                current_count += 1;
            }
        }
    }
    let listed = symbols.len() as u64;
    let state = scan_state(listed, on_disk, current_count, expected.is_some());
    Ok(ScanFigures {
        listed,
        on_disk,
        latest_date,
        current_count,
        bytes,
        state,
    })
}

/// The BT-605 state of a scanned dataset: Unknown with nothing to judge against (no listing
/// cached, no calendar to set the expected session), Stale when files exist but none reaches
/// the expected session, Partial when files exist for under `COMPLETE_SHARE` of the listed
/// symbols, else Current.
fn scan_state(listed: u64, on_disk: u64, current_count: u64, judged: bool) -> &'static str {
    if listed == 0 || !judged {
        STATE_UNKNOWN
    } else if on_disk > 0 && current_count == 0 {
        STATE_STALE
    } else if (on_disk as f64) < COMPLETE_SHARE * listed as f64 {
        STATE_PARTIAL
    } else {
        STATE_CURRENT
    }
}

/// Files and bytes under `dir`, symlinks not followed.
fn count_files(dir: &Path) -> (u64, u64) {
    let mut files = 0u64;
    let mut bytes = 0u64;
    let mut pending = vec![dir.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_file() {
                files += 1;
                bytes += entry.metadata().map(|m| m.len()).unwrap_or(0);
            } else if kind.is_dir() {
                pending.push(entry.path());
            }
        }
    }
    (files, bytes)
}

/// The folders under `root` that no claimed folder is at or under, each with its file count
/// and bytes; loose files directly under a visited folder count under that folder. A folder
/// with a claimed folder deeper inside is walked, not counted.
fn uncataloged_folders(root: &Path, claimed: &[PathBuf]) -> Vec<UncatalogedFolder> {
    let mut found = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        let mut loose_files = 0u64;
        let mut loose_bytes = 0u64;
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let path = entry.path();
            if kind.is_file() {
                loose_files += 1;
                loose_bytes += entry.metadata().map(|m| m.len()).unwrap_or(0);
            } else if kind.is_dir() {
                if claimed.iter().any(|c| c == &path) {
                    continue;
                }
                if claimed.iter().any(|c| c.starts_with(&path)) {
                    pending.push(path);
                    continue;
                }
                let (files, bytes) = count_files(&path);
                if files > 0 {
                    found.push(UncatalogedFolder {
                        folder: relative_to_root(root, &path),
                        files,
                        bytes,
                    });
                }
            }
        }
        if loose_files > 0 {
            found.push(UncatalogedFolder {
                folder: if dir == root {
                    ROOT_FOLDER.to_owned()
                } else {
                    relative_to_root(root, &dir)
                },
                files: loose_files,
                bytes: loose_bytes,
            });
        }
    }
    found.sort_by(|a, b| a.folder.cmp(&b.folder));
    found
}

/// Writes a dataset's figures from a scan that judged it.
fn store_figures(
    connection: &Connection,
    dataset_id: &str,
    scanned_at: &str,
    figures: &ScanFigures,
    uncataloged_json: &str,
) -> Result<()> {
    connection.execute(
        "INSERT OR REPLACE INTO dataset_scans
         (dataset_id, scanned_at, listed, on_disk, latest_date, current_count, bytes,
          uncataloged_json, state, error)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, NULL)",
        params![
            dataset_id,
            scanned_at,
            figures.listed as i64,
            figures.on_disk as i64,
            figures.latest_date,
            figures.current_count as i64,
            figures.bytes as i64,
            uncataloged_json,
            figures.state,
        ],
    )?;
    Ok(())
}

/// Records a scan that could not judge a dataset (its folder missing, or unreadable): the
/// state and the time move, the previous figures stay; a dataset never scanned gets zeros.
fn keep_figures(
    connection: &Connection,
    dataset_id: &str,
    scanned_at: &str,
    state: &str,
    uncataloged_json: &str,
    error: Option<&str>,
) -> Result<()> {
    let updated = connection.execute(
        "UPDATE dataset_scans
         SET scanned_at = ?2, state = ?3, uncataloged_json = ?4, error = ?5
         WHERE dataset_id = ?1",
        params![dataset_id, scanned_at, state, uncataloged_json, error],
    )?;
    if updated == 0 {
        connection.execute(
            "INSERT INTO dataset_scans
             (dataset_id, scanned_at, listed, on_disk, latest_date, current_count, bytes,
              uncataloged_json, state, error)
             VALUES (?1, ?2, 0, 0, NULL, 0, 0, ?3, ?4, ?5)",
            params![dataset_id, scanned_at, uncataloged_json, state, error],
        )?;
    }
    Ok(())
}

/// The scan job for one source: reads the datasets and their listings under one short
/// lock, walks the disk without it, and writes each dataset's row. A dataset whose root or
/// folder is missing is Unavailable with its previous figures; one whose folder cannot be
/// read is Failed with them and the reason.
fn run_scan(state: &AppState, source_id: &str) -> Result<()> {
    let (row, datasets, listings) = {
        let connection = state.database.lock().expect("database lock poisoned");
        let row = connection
            .query_row(
                &format!("SELECT {SOURCE_COLUMNS} FROM data_sources WHERE id = ?1"),
                [source_id],
                map_source,
            )
            .optional()?
            .ok_or_else(|| anyhow::anyhow!("no source {source_id:?}"))?;
        let datasets = load_datasets(&connection, source_id)?;
        let listings = datasets
            .iter()
            .map(|dataset| listed_symbols(&connection, dataset))
            .collect::<Result<Vec<_>>>()?;
        (row, datasets, listings)
    };
    let root = Path::new(&row.root);
    let root_exists = root.is_dir();
    let mut claimed: Vec<PathBuf> = datasets
        .iter()
        .map(|dataset| PathBuf::from(&dataset.folder))
        .collect();
    claimed.push(PathBuf::from(&row.catalog_dir));
    let uncataloged = if root_exists {
        uncataloged_folders(root, &claimed)
    } else {
        Vec::new()
    };
    let uncataloged_json = serde_json::to_string(&uncataloged)?;
    let daily_folders: Vec<PathBuf> = datasets
        .iter()
        .filter(|dataset| dataset.resolution == "daily")
        .map(|dataset| PathBuf::from(&dataset.folder))
        .collect();

    let scanned_at = Utc::now().to_rfc3339();
    for (dataset, symbols) in datasets.iter().zip(&listings) {
        let folder = Path::new(&dataset.folder);
        let outcome = if !root_exists || !folder.is_dir() {
            Err(None)
        } else {
            let expected = calendar_file(state, folder, &daily_folders)
                .and_then(|path| last_csv_row_date(&path))
                .and_then(|date| NaiveDate::parse_from_str(&date, "%Y-%m-%d").ok())
                .map(|session| ExpectedSession::for_resolution(session, &dataset.resolution));
            scan_dataset(folder, &dataset.exchange, symbols, expected)
                .map_err(|error| Some(format!("{error:#}")))
        };
        let connection = state.database.lock().expect("database lock poisoned");
        match outcome {
            Ok(figures) => store_figures(
                &connection,
                &dataset.id,
                &scanned_at,
                &figures,
                &uncataloged_json,
            )?,
            Err(None) => keep_figures(
                &connection,
                &dataset.id,
                &scanned_at,
                STATE_UNAVAILABLE,
                &uncataloged_json,
                None,
            )?,
            Err(Some(error)) => keep_figures(
                &connection,
                &dataset.id,
                &scanned_at,
                STATE_FAILED,
                &uncataloged_json,
                Some(&error),
            )?,
        }
    }
    Ok(())
}

/// The card's view of the last scan: when it ran and the Uncataloged folders it found,
/// from the newest dataset row.
fn last_scan_of(
    connection: &Connection,
    source_id: &str,
) -> Result<(Option<String>, Vec<UncatalogedFolder>)> {
    let newest = connection
        .query_row(
            "SELECT s.scanned_at, s.uncataloged_json FROM dataset_scans s
             JOIN datasets d ON d.id = s.dataset_id
             WHERE d.source_id = ?1 ORDER BY s.scanned_at DESC LIMIT 1",
            [source_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()?;
    Ok(match newest {
        Some((scanned_at, json)) => (
            Some(scanned_at),
            serde_json::from_str(&json).unwrap_or_default(),
        ),
        None => (None, Vec::new()),
    })
}

/// Whether a dataset holds data files: a file for any of its listed symbols; with no
/// listing cached, any file in its folder (it cannot tell whose, so it is refused).
fn dataset_holds_files(folder: &Path, exchange: &str, symbols: &[String]) -> bool {
    if symbols.is_empty() {
        return holds_files(folder);
    }
    symbols
        .iter()
        .any(|code| folder.join(symbol_file_name(code, exchange)).is_file())
}

fn validate_dataset_request(request: &CreateDatasetRequest) -> Result<()> {
    if request.exchange.trim().is_empty() {
        bail!("exchange is required");
    }
    if request.types.is_empty() || request.types.iter().any(|t| t.trim().is_empty()) {
        bail!("at least one type is required");
    }
    if request.resolution.trim().is_empty() {
        bail!("resolution is required");
    }
    if NaiveDate::parse_from_str(request.from_date.trim(), "%Y-%m-%d").is_err() {
        bail!("from_date must be a date (YYYY-MM-DD)");
    }
    Ok(())
}

/// Registers a dataset against the cached availability: the exchange must be in the
/// source's exchange list, the resolution among what the provider offers there, and every
/// type in the exchange's cached listing (409 when that listing is not cached yet); a
/// dataset of the source already covering one of the types on that exchange at that
/// resolution is 409. 201 with the row, state Unknown until a scan runs.
async fn create_dataset(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
    Json(request): Json<CreateDatasetRequest>,
) -> Result<(StatusCode, Json<DatasetRow>), ApiError> {
    let row = load_source_row(&state, &id)?
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, format!("no source {id:?}")))?;
    validate_dataset_request(&request)?;
    let exchange = request.exchange.trim().to_owned();
    let resolution = request.resolution.trim().to_owned();
    let mut types: Vec<String> = request.types.iter().map(|t| t.trim().to_owned()).collect();
    types.sort();
    types.dedup();
    let from_date = request.from_date.trim().to_owned();
    let include_delisted = request.include_delisted.unwrap_or(resolution == "daily");
    let folder = match request.folder.as_deref().map(str::trim) {
        Some(given) if !given.is_empty() => {
            let path = Path::new(given);
            if path.is_absolute() {
                path.to_path_buf()
            } else {
                Path::new(&row.root).join(path)
            }
        }
        _ => Path::new(&row.root).join(default_dataset_folder(&resolution)),
    };
    let folder = folder.display().to_string();

    let now = Utc::now();
    let dataset_id = format!("dataset-{}", now.format("%Y%m%dT%H%M%S%.6fZ"));
    {
        let connection = state.database.lock().expect("database lock poisoned");
        let resolutions: Option<String> = connection
            .query_row(
                "SELECT resolutions FROM provider_exchanges WHERE source_id = ?1 AND code = ?2",
                params![id, exchange],
                |r| r.get(0),
            )
            .optional()?;
        let Some(resolutions) = resolutions else {
            return Err(api_error(
                StatusCode::BAD_REQUEST,
                format!(
                    "{} lists no exchange {exchange:?}; refresh the availability first",
                    row.name
                ),
            ));
        };
        let offered: Vec<String> = serde_json::from_str(&resolutions).unwrap_or_default();
        if !offered.contains(&resolution) {
            return Err(api_error(
                StatusCode::BAD_REQUEST,
                format!(
                    "{exchange} offers no {resolution:?} bars; offered: {}",
                    offered.join(", ")
                ),
            ));
        }
        let mut statement = connection.prepare(
            "SELECT DISTINCT type FROM provider_listings
             WHERE source_id = ?1 AND exchange = ?2 ORDER BY type",
        )?;
        let cached: Vec<String> = statement
            .query_map(params![id, exchange], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        if cached.is_empty() {
            return Err(api_error(
                StatusCode::CONFLICT,
                format!("the listing of {exchange} is not cached; refresh it first"),
            ));
        }
        if let Some(unknown) = types.iter().find(|t| !cached.contains(t)) {
            return Err(api_error(
                StatusCode::BAD_REQUEST,
                format!(
                    "{exchange} lists no type {unknown:?}; cached: {}",
                    cached.join(", ")
                ),
            ));
        }
        for existing in load_datasets(&connection, &id)? {
            if existing.exchange != exchange || existing.resolution != resolution {
                continue;
            }
            if let Some(shared) = types.iter().find(|t| existing.types.contains(t)) {
                return Err(api_error(
                    StatusCode::CONFLICT,
                    format!(
                        "dataset {} already covers {shared} on {exchange} at {resolution}",
                        existing.id
                    ),
                ));
            }
        }
        connection.execute(
            "INSERT INTO datasets
             (id, source_id, exchange, types_json, resolution, from_date, folder,
              include_delisted, created_at, min_bulk_rows)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                dataset_id,
                id,
                exchange,
                serde_json::to_string(&types)?,
                resolution,
                from_date,
                folder,
                include_delisted,
                now.to_rfc3339(),
                request.min_bulk_rows.unwrap_or(DEFAULT_MIN_BULK_ROWS) as i64,
            ],
        )?;
    }
    let dataset = {
        let connection = state.database.lock().expect("database lock poisoned");
        load_dataset(&connection, &dataset_id)?
            .ok_or_else(|| anyhow::anyhow!("dataset {dataset_id:?} was not recorded"))?
    };
    Ok((StatusCode::CREATED, Json(dataset)))
}

/// Removes a dataset's registration and its scan row. Refused while a file exists for any
/// of its listed symbols: files are never deleted (decision 0022). 200 with the source card.
async fn delete_dataset(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<SourceCard>, ApiError> {
    let (dataset, symbols) = {
        let connection = state.database.lock().expect("database lock poisoned");
        let dataset = load_dataset(&connection, &id)?
            .ok_or_else(|| api_error(StatusCode::NOT_FOUND, format!("no dataset {id:?}")))?;
        let symbols = listed_symbols(&connection, &dataset)?;
        (dataset, symbols)
    };
    if let Some(running) = running_job_on(&state, &dataset.source_id)
        && running.dataset_id == dataset.id
    {
        return Err(api_error(
            StatusCode::CONFLICT,
            format!("job {} is running on dataset {}", running.id, dataset.id),
        ));
    }
    let folder = PathBuf::from(&dataset.folder);
    let exchange = dataset.exchange.clone();
    let occupied =
        tokio::task::spawn_blocking(move || dataset_holds_files(&folder, &exchange, &symbols))
            .await
            .context("dataset check failed")?;
    if occupied {
        return Err(api_error(
            StatusCode::CONFLICT,
            format!(
                "dataset {} still has files under {}; the console never deletes data files",
                dataset.id, dataset.folder
            ),
        ));
    }
    {
        let connection = state.database.lock().expect("database lock poisoned");
        connection.execute("DELETE FROM datasets WHERE id = ?1", [&id])?;
    }
    Ok(Json(load_source_card(&state, &dataset.source_id)?))
}

/// Starts the source's scan job in the background: 202 at once, the card says `scanning`
/// until the job has written every dataset's row. 409 while a scan of the source runs.
async fn scan_source(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Result<(StatusCode, Json<ScanAccepted>), ApiError> {
    let row = load_source_row(&state, &id)?
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, format!("no source {id:?}")))?;
    {
        let mut scans = state.scans.lock().expect("scan set poisoned");
        if !scans.insert(id.clone()) {
            return Err(api_error(
                StatusCode::CONFLICT,
                format!("a scan of source {:?} is already running", row.name),
            ));
        }
    }
    let worker = state.clone();
    let source_id = id.clone();
    tokio::spawn(async move {
        let inner = worker.clone();
        let scanned = source_id.clone();
        match tokio::task::spawn_blocking(move || run_scan(&inner, &scanned)).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => eprintln!("scan of source {source_id} failed: {error:#}"),
            Err(error) => eprintln!("scan of source {source_id} panicked: {error}"),
        }
        worker
            .scans
            .lock()
            .expect("scan set poisoned")
            .remove(&source_id);
    });
    Ok((
        StatusCode::ACCEPTED,
        Json(ScanAccepted {
            source_id: id,
            scanning: true,
        }),
    ))
}

// ---------------------------------------------------------------------------
// The native EOD download job (DS-08, BT-1205; decisions 0020 and 0022). The job itself is
// `tessera::provider::jobs::eod`, run over the source's adapter; the service maps the
// dataset and its cached listing onto the job's input, keeps one job per source, refuses a
// folder before any call, records progress and the outcome in `dataset_jobs`, writes the
// log under data/ui/logs/, and, after the job, refreshes the source's usage and rescans it.
// ---------------------------------------------------------------------------

/// The job kind `dataset_jobs.kind` names for a daily dataset's update.
const JOB_KIND_EOD: &str = "eod";

/// A download job as the console shows it.
#[derive(Debug, Clone, Serialize)]
struct DatasetJobRecord {
    id: String,
    dataset_id: String,
    source_id: String,
    kind: String,
    /// Queued, Running, Complete, or Failed.
    state: String,
    percent: u8,
    created_at: String,
    started_at: Option<String>,
    finished_at: Option<String>,
    calls: u64,
    /// Files created by the backfill.
    added: u64,
    /// Files appended to or replaced.
    updated: u64,
    /// Symbols the job did not bring current, each with its reason.
    skipped: Vec<Skipped>,
    /// The call estimate the budget judged; `None` when the job was refused before one.
    estimate: Option<Estimate>,
    error: Option<String>,
    /// Relative to the service root; `GET /api/datasets/jobs/{id}/log` serves it.
    log_path: String,
}

const DATASET_JOB_COLUMNS: &str = "j.id, j.dataset_id, d.source_id, j.kind, j.state, \
     j.percent, j.created_at, j.started_at, j.finished_at, j.calls, j.added, j.updated, \
     j.skipped_json, j.estimate_json, j.error, j.log_path";

fn map_dataset_job(row: &rusqlite::Row<'_>) -> rusqlite::Result<DatasetJobRecord> {
    let skipped_json: String = row.get(12)?;
    let estimate_json: Option<String> = row.get(13)?;
    Ok(DatasetJobRecord {
        id: row.get(0)?,
        dataset_id: row.get(1)?,
        source_id: row.get(2)?,
        kind: row.get(3)?,
        state: row.get(4)?,
        percent: row.get::<_, i64>(5)?.clamp(0, 100) as u8,
        created_at: row.get(6)?,
        started_at: row.get(7)?,
        finished_at: row.get(8)?,
        calls: row.get::<_, i64>(9)?.max(0) as u64,
        added: row.get::<_, i64>(10)?.max(0) as u64,
        updated: row.get::<_, i64>(11)?.max(0) as u64,
        skipped: serde_json::from_str(&skipped_json).unwrap_or_default(),
        estimate: estimate_json.and_then(|text| serde_json::from_str(&text).ok()),
        error: row.get(14)?,
        log_path: row.get(15)?,
    })
}

fn load_dataset_job(connection: &Connection, id: &str) -> Result<Option<DatasetJobRecord>> {
    Ok(connection
        .query_row(
            &format!(
                "SELECT {DATASET_JOB_COLUMNS} FROM dataset_jobs j
                 JOIN datasets d ON d.id = j.dataset_id WHERE j.id = ?1"
            ),
            [id],
            map_dataset_job,
        )
        .optional()?)
}

/// The newest job on a dataset.
fn last_job_of(connection: &Connection, dataset_id: &str) -> Result<Option<DatasetJobRecord>> {
    Ok(connection
        .query_row(
            &format!(
                "SELECT {DATASET_JOB_COLUMNS} FROM dataset_jobs j
                 JOIN datasets d ON d.id = j.dataset_id
                 WHERE j.dataset_id = ?1 ORDER BY j.created_at DESC, j.id DESC LIMIT 1"
            ),
            [dataset_id],
            map_dataset_job,
        )
        .optional()?)
}

/// The job holding the source's lock, if one runs.
fn running_job_on(state: &AppState, source_id: &str) -> Option<RunningJob> {
    state
        .dataset_jobs
        .lock()
        .expect("job set poisoned")
        .get(source_id)
        .cloned()
}

/// The dataset's symbols with their listings: the rows of its types on its exchange, the
/// delisted ones when the dataset includes them, one per code (the active row first when a
/// code is in both lists).
fn dataset_symbols(connection: &Connection, dataset: &DatasetRow) -> Result<Vec<DatasetSymbol>> {
    if dataset.types.is_empty() {
        return Ok(Vec::new());
    }
    let placeholders = (0..dataset.types.len())
        .map(|i| format!("?{}", i + 4))
        .collect::<Vec<_>>()
        .join(", ");
    let mut statement = connection.prepare(&format!(
        "SELECT code, name, type, currency, country, venue, delisted FROM provider_listings
         WHERE source_id = ?1 AND exchange = ?2 AND (delisted = 0 OR ?3)
           AND type IN ({placeholders})
         ORDER BY code, delisted"
    ))?;
    let mut values: Vec<&dyn rusqlite::ToSql> = vec![
        &dataset.source_id,
        &dataset.exchange,
        &dataset.include_delisted,
    ];
    values.extend(dataset.types.iter().map(|t| t as &dyn rusqlite::ToSql));
    let rows = statement
        .query_map(values.as_slice(), |row| {
            Ok(DatasetSymbol {
                listing: Listing {
                    code: row.get(0)?,
                    name: row.get(1)?,
                    kind: row.get(2)?,
                    currency: row.get(3)?,
                    country: row.get(4)?,
                    venue: row.get(5)?,
                },
                delisted: row.get::<_, i64>(6)? != 0,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut symbols: Vec<DatasetSymbol> = Vec::with_capacity(rows.len());
    for row in rows {
        if symbols
            .last()
            .is_none_or(|last| last.listing.code != row.listing.code)
        {
            symbols.push(row);
        }
    }
    Ok(symbols)
}

/// The exchange's active listing, every type: what the catalog files are regenerated from.
fn exchange_listing(
    connection: &Connection,
    source_id: &str,
    exchange: &str,
) -> Result<Vec<Listing>> {
    let mut statement = connection.prepare(
        "SELECT code, name, type, currency, country, venue FROM provider_listings
         WHERE source_id = ?1 AND exchange = ?2 AND delisted = 0 ORDER BY code",
    )?;
    let rows = statement
        .query_map(params![source_id, exchange], |row| {
            Ok(Listing {
                code: row.get(0)?,
                name: row.get(1)?,
                kind: row.get(2)?,
                currency: row.get(3)?,
                country: row.get(4)?,
                venue: row.get(5)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// The calendar symbol's code when it is listed on `exchange` (`SPY.US` on `US` is `SPY`);
/// `None` on another exchange, where a bulk day cannot be checked for it.
fn calendar_code_for(calendar_symbol: &str, exchange: &str) -> Option<String> {
    let (code, suffix) = calendar_symbol.rsplit_once('.')?;
    (suffix == exchange && !code.is_empty()).then(|| code.to_owned())
}

/// The last session a run fetches: today in New York, where the exchanges the console
/// serves close; a session the provider has not published yet answers with no bars and is
/// skipped, so asking a day early costs one call and nothing else.
fn today_in_new_york() -> NaiveDate {
    Utc::now()
        .with_timezone(&chrono_tz::America::New_York)
        .date_naive()
}

/// Queues the dataset's download job: 202 with the record, the job running in the
/// background. Refused, before any provider call, with 409 while a job runs on the source
/// (naming it), for a root that is not mounted or a folder that is missing, not a folder, or
/// not writable (the job never creates it), and when no token is on file; then, after the
/// source's usage is refreshed, with 409 when the provider has not reported usage (a job
/// is governed by it, decision 0022). Only daily datasets have a job yet.
async fn start_dataset_update(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Result<(StatusCode, Json<DatasetJobRecord>), ApiError> {
    let dataset = {
        let connection = state.database.lock().expect("database lock poisoned");
        load_dataset(&connection, &id)?
            .ok_or_else(|| api_error(StatusCode::NOT_FOUND, format!("no dataset {id:?}")))?
    };
    if dataset.resolution != "daily" {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            format!(
                "the EOD job updates daily datasets; {} datasets have no job yet",
                dataset.resolution
            ),
        ));
    }
    let row = load_source_row(&state, &dataset.source_id)?.ok_or_else(|| {
        api_error(
            StatusCode::NOT_FOUND,
            format!("no source {:?}", dataset.source_id),
        )
    })?;
    let busy = |running: RunningJob| {
        api_error(
            StatusCode::CONFLICT,
            format!(
                "job {} is running on source {:?} (dataset {}); one job per source at a time",
                running.id, row.name, running.dataset_id
            ),
        )
    };
    if let Some(running) = running_job_on(&state, &row.id) {
        return Err(busy(running));
    }
    if !Path::new(&row.root).is_dir() {
        return Err(api_error(
            StatusCode::CONFLICT,
            format!(
                "root {} is not mounted; the job never creates a folder",
                row.root
            ),
        ));
    }
    eod_job::check_folder(Path::new(&dataset.folder))
        .map_err(|refusal| api_error(StatusCode::CONFLICT, refusal))?;
    let adapter = SourceAdapter::from_file(&state, &row)?;

    refresh_source_usage(&state, &row, true).await?;
    let row = load_source_row(&state, &row.id)?
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, format!("no source {:?}", row.id)))?;
    let Some(budget) = budget_of(&row) else {
        return Err(api_error(
            StatusCode::CONFLICT,
            format!(
                "the provider has not reported usage for source {:?} ({}); a job starts only \
                 from reported usage",
                row.name,
                row.verify_message.as_deref().unwrap_or("not verified")
            ),
        ));
    };

    let now = Utc::now();
    let job_id = format!("job-{}", now.format("%Y%m%dT%H%M%S%.6fZ"));
    let log_path = format!("data/ui/logs/{job_id}.log");
    {
        let mut jobs = state.dataset_jobs.lock().expect("job set poisoned");
        if let Some(running) = jobs.get(&row.id) {
            return Err(busy(running.clone()));
        }
        jobs.insert(
            row.id.clone(),
            RunningJob {
                id: job_id.clone(),
                dataset_id: dataset.id.clone(),
            },
        );
    }
    let inserted = {
        let connection = state.database.lock().expect("database lock poisoned");
        connection.execute(
            "INSERT INTO dataset_jobs (id, dataset_id, kind, state, created_at, log_path)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                job_id,
                dataset.id,
                JOB_KIND_EOD,
                JobState::Queued.as_str(),
                now.to_rfc3339(),
                log_path
            ],
        )
    };
    if let Err(error) = inserted {
        state
            .dataset_jobs
            .lock()
            .expect("job set poisoned")
            .remove(&row.id);
        return Err(anyhow::Error::from(error).context("record the job").into());
    }
    let worker = state.clone();
    let worker_id = job_id.clone();
    tokio::spawn(async move {
        run_dataset_job(worker, worker_id, dataset, row, adapter, budget).await;
    });
    let record = {
        let connection = state.database.lock().expect("database lock poisoned");
        load_dataset_job(&connection, &job_id)?
            .ok_or_else(|| anyhow::anyhow!("job {job_id:?} was not recorded"))?
    };
    Ok((StatusCode::ACCEPTED, Json(record)))
}

/// Writes a job's progress figures.
fn record_job_progress(state: &AppState, job_id: &str, progress: eod_job::Progress) {
    let connection = state.database.lock().expect("database lock poisoned");
    let _ = connection.execute(
        "UPDATE dataset_jobs SET percent = ?2, calls = ?3, added = ?4, updated = ?5 WHERE id = ?1",
        params![
            job_id,
            progress.percent as i64,
            progress.calls as i64,
            progress.added as i64,
            progress.updated as i64
        ],
    );
}

/// Marks a job finished with its outcome.
fn record_job_outcome(
    state: &AppState,
    job_id: &str,
    outcome: &eod_job::Outcome,
    finished_at: &str,
) -> Result<()> {
    let connection = state.database.lock().expect("database lock poisoned");
    connection.execute(
        "UPDATE dataset_jobs
         SET state = ?2, finished_at = ?3, calls = ?4, added = ?5, updated = ?6,
             skipped_json = ?7, estimate_json = ?8, error = ?9,
             percent = CASE WHEN ?2 = 'Complete' THEN 100 ELSE percent END
         WHERE id = ?1",
        params![
            job_id,
            outcome.state.as_str(),
            finished_at,
            outcome.calls as i64,
            outcome.added as i64,
            outcome.updated as i64,
            serde_json::to_string(&outcome.skipped)?,
            outcome
                .estimate
                .map(|estimate| serde_json::to_string(&estimate))
                .transpose()?,
            outcome.error,
        ],
    )?;
    Ok(())
}

/// Runs the job over the adapter and records it; a failure of the service's own steps
/// (reading the listing, opening the log) is the job's error. Whatever happened, the
/// source's usage is refreshed from the provider, the source is rescanned so the dataset's
/// figures follow the files, and the source's lock is released last.
async fn run_dataset_job(
    state: AppState,
    job_id: String,
    dataset: DatasetRow,
    source: SourceRow,
    adapter: SourceAdapter,
    budget: CallBudget,
) {
    let outcome = execute_dataset_job(&state, &job_id, &dataset, &source, &adapter, budget).await;
    let finished_at = Utc::now().to_rfc3339();
    let recorded = match outcome {
        Ok(outcome) => record_job_outcome(&state, &job_id, &outcome, &finished_at),
        Err(error) => {
            let connection = state.database.lock().expect("database lock poisoned");
            connection
                .execute(
                    "UPDATE dataset_jobs SET state = 'Failed', finished_at = ?2, error = ?3
                     WHERE id = ?1",
                    params![job_id, finished_at, format!("{error:#}")],
                )
                .map(|_| ())
                .map_err(anyhow::Error::from)
        }
    };
    if let Err(error) = recorded {
        eprintln!("job {job_id} could not be recorded: {error:#}");
    }
    if let Err(error) = refresh_source_usage(&state, &source, true).await {
        eprintln!("usage refresh after job {job_id} failed: {error:#}");
    }
    let rescan = state
        .scans
        .lock()
        .expect("scan set poisoned")
        .insert(source.id.clone());
    if rescan {
        let inner = state.clone();
        let source_id = source.id.clone();
        match tokio::task::spawn_blocking(move || run_scan(&inner, &source_id)).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => eprintln!("scan after job {job_id} failed: {error:#}"),
            Err(error) => eprintln!("scan after job {job_id} panicked: {error}"),
        }
        state
            .scans
            .lock()
            .expect("scan set poisoned")
            .remove(&source.id);
    }
    state
        .dataset_jobs
        .lock()
        .expect("job set poisoned")
        .remove(&source.id);
}

/// Marks the job running, builds the job's input from the dataset and its cached listing,
/// opens the log, and runs the job with progress written to the row as it moves.
async fn execute_dataset_job(
    state: &AppState,
    job_id: &str,
    dataset: &DatasetRow,
    source: &SourceRow,
    adapter: &SourceAdapter,
    mut budget: CallBudget,
) -> Result<eod_job::Outcome> {
    {
        let connection = state.database.lock().expect("database lock poisoned");
        connection.execute(
            "UPDATE dataset_jobs SET state = ?2, started_at = ?3 WHERE id = ?1",
            params![job_id, JobState::Running.as_str(), Utc::now().to_rfc3339()],
        )?;
    }
    let (symbols, catalog) = {
        let connection = state.database.lock().expect("database lock poisoned");
        (
            dataset_symbols(&connection, dataset)?,
            exchange_listing(&connection, &dataset.source_id, &dataset.exchange)?,
        )
    };
    let from_date = NaiveDate::parse_from_str(&dataset.from_date, "%Y-%m-%d")
        .with_context(|| format!("from_date {:?} is not a date", dataset.from_date))?;
    let input = EodJobInput {
        exchange: dataset.exchange.clone(),
        folder: PathBuf::from(&dataset.folder),
        catalog_dir: PathBuf::from(&source.catalog_dir),
        from_date,
        through: today_in_new_york(),
        min_bulk_rows: dataset.min_bulk_rows,
        calendar_code: calendar_code_for(&state.local.data.calendar_symbol, &dataset.exchange),
        symbols,
        catalog,
    };
    let log_path = state.root.join(format!("data/ui/logs/{job_id}.log"));
    if let Some(parent) = log_path.parent() {
        fs::create_dir_all(parent).context("create the log folder")?;
    }
    let mut log = fs::File::create(&log_path).context("open the job log")?;
    let progress_state = state.clone();
    let progress_id = job_id.to_owned();
    let mut last_written: Option<(std::time::Instant, u8)> = None;
    let mut on_progress = move |progress: eod_job::Progress| {
        let due = last_written.is_none_or(|(at, percent)| {
            percent != progress.percent || at.elapsed() >= std::time::Duration::from_secs(1)
        });
        if due {
            record_job_progress(&progress_state, &progress_id, progress);
            last_written = Some((std::time::Instant::now(), progress.percent));
        }
    };
    Ok(eod_job::run(adapter, &input, &mut budget, &mut log, &mut on_progress).await)
}

async fn get_dataset_job(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<DatasetJobRecord>, ApiError> {
    let connection = state.database.lock().expect("database lock poisoned");
    let record = load_dataset_job(&connection, &id)?
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, format!("no job {id:?}")))?;
    Ok(Json(record))
}

/// The job's log as a text download; empty while the job has not started writing it.
async fn get_dataset_job_log(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Result<axum::response::Response, ApiError> {
    let record = {
        let connection = state.database.lock().expect("database lock poisoned");
        load_dataset_job(&connection, &id)?
            .ok_or_else(|| api_error(StatusCode::NOT_FOUND, format!("no job {id:?}")))?
    };
    let text = fs::read_to_string(state.root.join(&record.log_path)).unwrap_or_default();
    Ok((
        StatusCode::OK,
        [
            (
                axum::http::header::CONTENT_TYPE,
                HeaderValue::from_static("text/plain; charset=utf-8"),
            ),
            (
                axum::http::header::CONTENT_DISPOSITION,
                HeaderValue::from_str(&format!("attachment; filename=\"{}.log\"", record.id))
                    .unwrap_or_else(|_| HeaderValue::from_static("attachment")),
            ),
        ],
        text,
    )
        .into_response())
}

// ---------------------------------------------------------------------------
// Feature studies on the tick lake (feature vs forward return).
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StudyRecord {
    id: String,
    name: String,
    status: String,
    start_date: String,
    end_date: String,
    config_json: String,
    created_at: String,
    finished_at: Option<String>,
    error: Option<String>,
    artifact_dir: String,
}

#[derive(Debug, Clone, Deserialize)]
struct CreateStudyRequest {
    name: String,
    start_date: String,
    end_date: String,
    symbols: Vec<String>,
    #[serde(default = "default_study_step")]
    step_secs: u32,
    /// `daily`, `5m`, or `1m` for CSV bars through the SDK loader; absent means lake bars on
    /// `step_secs`.
    #[serde(default)]
    resolution: Option<String>,
    #[serde(default)]
    features: Vec<String>,
    #[serde(default)]
    horizons: Vec<usize>,
    #[serde(default)]
    decision_delay_bars: usize,
    /// `time_series` (default) or `cross_sectional`.
    #[serde(default)]
    mode: Option<String>,
    /// Bars either side of an event in the event-study path (default 20).
    #[serde(default)]
    event_window: Option<usize>,
    /// Accepted feature expressions regressed out before the incremental IC.
    #[serde(default)]
    accepted: Vec<String>,
    /// `return` (default), `realized_variance`, `realized_vol`, `abs_move`,
    /// `spread_change`, `fair_value_residual`, or `microprice_residual`.
    #[serde(default)]
    target: Option<String>,
}

fn default_study_step() -> u32 {
    1
}

#[derive(Debug, Serialize)]
struct StudyDetailResponse {
    study: StudyRecord,
    result: Option<serde_json::Value>,
}

fn query_studies(connection: &Connection, limit: usize) -> Result<Vec<StudyRecord>> {
    let mut statement = connection.prepare(
        "SELECT id, name, status, start_date, end_date, config_json, created_at, finished_at, error, artifact_dir
         FROM studies ORDER BY created_at DESC LIMIT ?1",
    )?;
    let rows = statement.query_map([limit as i64], |row| {
        Ok(StudyRecord {
            id: row.get(0)?,
            name: row.get(1)?,
            status: row.get(2)?,
            start_date: row.get(3)?,
            end_date: row.get(4)?,
            config_json: row.get(5)?,
            created_at: row.get(6)?,
            finished_at: row.get(7)?,
            error: row.get(8)?,
            artifact_dir: row.get(9)?,
        })
    })?;
    Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
}

async fn list_studies(State(state): State<AppState>) -> Result<Json<Vec<StudyRecord>>, ApiError> {
    let connection = state.database.lock().expect("database lock poisoned");
    Ok(Json(query_studies(&connection, 50)?))
}

async fn lake_instruments(
    State(state): State<AppState>,
) -> Result<Json<Vec<tessera::lake::LakeInstrument>>, ApiError> {
    let Some(lake) = state.local.data.lake_dir.clone() else {
        return Ok(Json(Vec::new()));
    };
    let instruments = tokio::task::spawn_blocking(move || tessera::lake::discover(&lake))
        .await
        .context("lake discovery task failed")??;
    Ok(Json(instruments))
}

async fn study_detail(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<StudyDetailResponse>, ApiError> {
    let study = {
        let connection = state.database.lock().expect("database lock poisoned");
        query_studies(&connection, 1000)?
            .into_iter()
            .find(|study| study.id == id)
            .context("unknown study")?
    };
    let path = state.root.join(&study.artifact_dir).join("study.json");
    let result = tokio::task::spawn_blocking(move || {
        fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
    })
    .await
    .context("study read task failed")?;
    Ok(Json(StudyDetailResponse { study, result }))
}

async fn create_study(
    State(state): State<AppState>,
    Json(request): Json<CreateStudyRequest>,
) -> Result<(StatusCode, Json<StudyRecord>), ApiError> {
    let (config, start, end) = validate_study_request(&state, &request)?;
    let stamp = Utc::now().format("%Y%m%dT%H%M%S%.6fZ").to_string();
    let id = format!("study-{stamp}");
    let artifact_dir = format!("artifacts/studies/{stamp}");
    let record = StudyRecord {
        id: id.clone(),
        name: request.name.trim().to_owned(),
        status: "running".to_owned(),
        start_date: request.start_date.clone(),
        end_date: request.end_date.clone(),
        config_json: serde_json::to_string(&config)?,
        created_at: Utc::now().to_rfc3339(),
        finished_at: None,
        error: None,
        artifact_dir: artifact_dir.clone(),
    };
    insert_study(&state, &record)?;
    spawn_study(state, id, artifact_dir, config, start, end);
    Ok((StatusCode::ACCEPTED, Json(record)))
}

fn validate_study_request(
    state: &AppState,
    request: &CreateStudyRequest,
) -> Result<(tessera::study::StudyConfig, NaiveDate, NaiveDate)> {
    let grid = tessera::study::Grid::parse(request.resolution.as_deref(), request.step_secs)?;
    let data = &state.local.data;
    let lake_dir = match grid {
        tessera::study::Grid::Lake { .. } => data
            .lake_dir
            .clone()
            .context("lake studies need lake_dir in local.toml")?,
        _ => data.lake_dir.clone().unwrap_or_default(),
    };
    anyhow::ensure!(!request.name.trim().is_empty(), "name the study");
    anyhow::ensure!(!request.symbols.is_empty(), "select at least one symbol");
    let symbols: Vec<String> = if grid.has_book() {
        for symbol in &request.symbols {
            anyhow::ensure!(
                tessera::lake::is_lake_symbol(symbol),
                "{symbol} is not an EXCHANGE:SYMBOL lake instrument"
            );
        }
        request.symbols.clone()
    } else {
        // CSV grids take plain symbols; the SDK loader reads `<dir>/<SYMBOL>.csv`.
        let dir = match grid {
            tessera::study::Grid::FiveMinute => &data.five_minute_dir,
            tessera::study::Grid::OneMinute => &data.one_minute_dir,
            _ => &data.daily_dir,
        };
        let mut symbols = Vec::new();
        for symbol in &request.symbols {
            let symbol = normalize_sdk_symbol(symbol)?;
            anyhow::ensure!(
                dir.join(format!("{symbol}.csv")).is_file(),
                "{symbol} has no {} file under {}",
                grid.label(),
                dir.display()
            );
            symbols.push(symbol);
        }
        symbols
    };
    let start = NaiveDate::parse_from_str(&request.start_date, "%Y-%m-%d")
        .context("start_date must use YYYY-MM-DD")?;
    let end = NaiveDate::parse_from_str(&request.end_date, "%Y-%m-%d")
        .context("end_date must use YYYY-MM-DD")?;
    anyhow::ensure!(start <= end, "start_date must be on or before end_date");
    if grid.has_book() {
        anyhow::ensure!(
            request.step_secs > 0 && request.step_secs < 60 && 60 % request.step_secs == 0,
            "step_secs must divide a minute (1, 2, 5, 10, 15, 30)"
        );
    }
    let mut config = tessera::study::StudyConfig {
        lake_dir,
        symbols,
        step_secs: request.step_secs,
        resolution: request
            .resolution
            .as_deref()
            .map(str::trim)
            .filter(|r| !r.is_empty() && *r != "lake")
            .map(str::to_owned),
        daily_dir: data.daily_dir.clone(),
        five_minute_dir: data.five_minute_dir.clone(),
        one_minute_dir: data.one_minute_dir.clone(),
        calendar_symbol: Some(data.calendar_symbol.clone()),
        session: tessera::sdk::runner::SessionKind::Regular,
        features: if request.features.is_empty() {
            tessera::study::FEATURES
                .iter()
                .map(|f| (*f).to_owned())
                .collect()
        } else {
            request.features.clone()
        },
        horizons: if request.horizons.is_empty() {
            vec![1, 5, 30, 60]
        } else {
            request.horizons.clone()
        },
        decision_delay_bars: request.decision_delay_bars,
        buckets: 10,
        target: match request.target.as_deref().map(str::trim) {
            Some(text) if !text.is_empty() => tessera::study::Target::parse(text)?,
            _ => tessera::study::Target::default(),
        },
        series: data.series.clone(),
        lake_series: true,
        mode: match request.mode.as_deref() {
            Some(text) => tessera::study::StudyMode::parse(text)?,
            None => tessera::study::StudyMode::default(),
        },
        // `agg daily` features on the daily grid lift from the finest intraday library present.
        intraday_source: if data.one_minute_dir.is_dir() {
            Some("1m".to_owned())
        } else if data.five_minute_dir.is_dir() {
            Some("5m".to_owned())
        } else {
            None
        },
        event_window: request.event_window.unwrap_or(20).clamp(1, 500),
        accepted: request
            .accepted
            .iter()
            .map(|a| a.trim().to_owned())
            .filter(|a| !a.is_empty())
            .collect(),
    };
    let names = study_base_names(data, grid.has_book());
    // Every promoted preset joins the accepted set (WB-09); one that cannot run on this grid
    // (an order-book or lake-feed feature on CSV bars) is left out rather than blocking it.
    let promoted: Vec<String> = {
        let connection = state.database.lock().expect("database lock poisoned");
        promoted_expressions(&connection)?
    }
    .into_iter()
    .filter(|expression| {
        tessera::feature_expr::parse_with(expression, &names)
            .is_ok_and(|expr| grid.has_book() || !tessera::feature_expr::needs_book(&expr))
    })
    .collect();
    config.accepted = merge_accepted(&config.accepted, &promoted);
    for feature in config.features.iter().chain(&config.accepted) {
        tessera::feature_expr::parse_with(feature, &names)
            .with_context(|| format!("feature expression {feature:?}"))?;
    }
    Ok((config, start, end))
}

/// A base beyond the bar that the studies form lists as a checkbox (WB-13): a series declared
/// in `[[data.series]]`, or one of the lake side feeds.
#[derive(Debug, Clone, Serialize)]
struct StudySeriesInfo {
    name: String,
    /// `level`, `event`, or `lake`.
    kind: String,
    /// Where the values come from: the declared file, or the lake feed and column.
    source: String,
    /// The availability rule the join honours.
    availability: String,
    /// Only the tick lake carries it: greyed out on CSV grids.
    lake_only: bool,
}

/// Every base beyond the bar: the declared series in their configured order, then the lake
/// side feeds.
fn study_series_catalog(declared: &[tessera::series::SeriesSpec]) -> Vec<StudySeriesInfo> {
    let mut out: Vec<StudySeriesInfo> = declared
        .iter()
        .map(|spec| StudySeriesInfo {
            name: spec.name.clone(),
            kind: match spec.kind {
                tessera::series::SeriesKind::Level => "level".to_owned(),
                tessera::series::SeriesKind::Event => "event".to_owned(),
            },
            source: spec.path.display().to_string(),
            availability: match (&spec.available_at_column, spec.publication_lag_secs) {
                (Some(column), _) => format!("as-of the {column} column"),
                (None, lag) if lag > 0 => format!("nominal time + {lag} s"),
                (None, _) => "at the nominal time".to_owned(),
            },
            lake_only: false,
        })
        .collect();
    out.extend(
        tessera::study::LAKE_SERIES
            .iter()
            .map(|(name, feed, column)| StudySeriesInfo {
                name: (*name).to_owned(),
                kind: "lake".to_owned(),
                source: format!("lake feed {feed}.{column}"),
                availability: "as-of receipt (recvTimestampMicros)".to_owned(),
                lake_only: true,
            }),
    );
    out
}

async fn list_study_series(
    State(state): State<AppState>,
) -> Result<Json<Vec<StudySeriesInfo>>, ApiError> {
    Ok(Json(study_series_catalog(&state.local.data.series)))
}

/// The bases an expression may use beyond the bar: the declared series, plus the lake side
/// feeds when the grid has an order book.
fn study_base_names(data: &tessera::local_config::DataLibrary, book_grid: bool) -> Vec<String> {
    let mut names: Vec<String> = data.series.iter().map(|s| s.name.clone()).collect();
    if book_grid {
        names.extend(
            tessera::study::LAKE_SERIES
                .iter()
                .map(|(n, _, _)| (*n).to_owned()),
        );
    }
    names
}

/// A named feature expression in the catalog (WB-09). Promoted presets form the accepted set
/// every study regresses its candidates against before their incremental IC.
#[derive(Debug, Clone, Serialize)]
struct FeaturePresetRecord {
    id: String,
    name: String,
    expression: String,
    note: String,
    accepted: bool,
    created_at: String,
    promoted_at: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SaveFeaturePresetRequest {
    name: String,
    expression: String,
    #[serde(default)]
    note: String,
    /// Save it straight into the accepted set.
    #[serde(default)]
    accepted: bool,
}

#[derive(Debug, Deserialize)]
struct PromoteFeatureRequest {
    accepted: bool,
}

const FEATURE_PRESET_COLUMNS: &str =
    "id, name, expression, note, accepted, created_at, promoted_at";

fn map_feature_preset(row: &rusqlite::Row<'_>) -> rusqlite::Result<FeaturePresetRecord> {
    Ok(FeaturePresetRecord {
        id: row.get(0)?,
        name: row.get(1)?,
        expression: row.get(2)?,
        note: row.get(3)?,
        accepted: row.get::<_, i64>(4)? != 0,
        created_at: row.get(5)?,
        promoted_at: row.get(6)?,
    })
}

/// Every preset, the accepted ones first, then by name.
fn query_feature_presets(connection: &Connection) -> Result<Vec<FeaturePresetRecord>> {
    let mut statement = connection.prepare(&format!(
        "SELECT {FEATURE_PRESET_COLUMNS} FROM feature_presets
         ORDER BY accepted DESC, name COLLATE NOCASE"
    ))?;
    let rows = statement.query_map([], map_feature_preset)?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

fn query_feature_preset(connection: &Connection, id: &str) -> Result<FeaturePresetRecord> {
    connection
        .query_row(
            &format!("SELECT {FEATURE_PRESET_COLUMNS} FROM feature_presets WHERE id = ?1"),
            [id],
            map_feature_preset,
        )
        .optional()?
        .with_context(|| format!("unknown feature preset {id}"))
}

/// Saves a named expression. An existing name is updated in place and keeps its promotion,
/// so a promoted feature can be refined without falling out of the accepted set.
fn save_feature_preset(
    connection: &Connection,
    name: &str,
    expression: &str,
    note: &str,
    accepted: bool,
) -> Result<FeaturePresetRecord> {
    let name = name.trim();
    let expression = expression.trim();
    anyhow::ensure!(
        !name.is_empty() && name.len() <= 80,
        "preset name must contain 1 to 80 characters"
    );
    anyhow::ensure!(!expression.is_empty(), "the preset needs an expression");
    let now = Utc::now().to_rfc3339();
    let existing: Option<String> = connection
        .query_row(
            "SELECT id FROM feature_presets WHERE name = ?1",
            [name],
            |row| row.get(0),
        )
        .optional()?;
    let id = match existing {
        Some(id) => {
            connection.execute(
                "UPDATE feature_presets
                 SET expression = ?2, note = ?3,
                     promoted_at = CASE WHEN accepted = 0 AND ?4 THEN ?5 ELSE promoted_at END,
                     accepted = MAX(accepted, ?4)
                 WHERE id = ?1",
                params![id, expression, note.trim(), accepted, now],
            )?;
            id
        }
        None => {
            let id = format!("feature-{}", Utc::now().format("%Y%m%dT%H%M%S%.6fZ"));
            connection.execute(
                "INSERT INTO feature_presets
                 (id, name, expression, note, accepted, created_at, promoted_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    id,
                    name,
                    expression,
                    note.trim(),
                    accepted,
                    now,
                    accepted.then(|| now.clone())
                ],
            )?;
            id
        }
    };
    query_feature_preset(connection, &id)
}

/// Promotes a preset into the accepted set, or takes it back out.
fn set_feature_promoted(
    connection: &Connection,
    id: &str,
    accepted: bool,
) -> Result<FeaturePresetRecord> {
    let promoted_at = accepted.then(|| Utc::now().to_rfc3339());
    let changed = connection.execute(
        "UPDATE feature_presets SET accepted = ?2, promoted_at = ?3 WHERE id = ?1",
        params![id, accepted, promoted_at],
    )?;
    anyhow::ensure!(changed == 1, "unknown feature preset {id}");
    query_feature_preset(connection, id)
}

fn delete_feature_preset(connection: &Connection, id: &str) -> Result<()> {
    let changed = connection.execute("DELETE FROM feature_presets WHERE id = ?1", [id])?;
    anyhow::ensure!(changed == 1, "unknown feature preset {id}");
    Ok(())
}

/// The expressions of every promoted preset, oldest promotion first.
fn promoted_expressions(connection: &Connection) -> Result<Vec<String>> {
    let mut statement = connection.prepare(
        "SELECT expression FROM feature_presets WHERE accepted = 1
         ORDER BY promoted_at, name COLLATE NOCASE",
    )?;
    let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// A study's accepted set: what the request asked for, then every promoted preset, trimmed
/// and once each.
fn merge_accepted(requested: &[String], promoted: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for expression in requested.iter().chain(promoted) {
        let expression = expression.trim();
        if !expression.is_empty() && !out.iter().any(|seen| seen == expression) {
            out.push(expression.to_owned());
        }
    }
    out
}

async fn list_feature_presets(
    State(state): State<AppState>,
) -> Result<Json<Vec<FeaturePresetRecord>>, ApiError> {
    let connection = state.database.lock().expect("database lock poisoned");
    Ok(Json(query_feature_presets(&connection)?))
}

async fn create_feature_preset(
    State(state): State<AppState>,
    Json(request): Json<SaveFeaturePresetRequest>,
) -> Result<(StatusCode, Json<FeaturePresetRecord>), ApiError> {
    // The expression must parse with every base the widest grid offers.
    let names = study_base_names(&state.local.data, true);
    let expression = request.expression.trim();
    tessera::feature_expr::parse_with(expression, &names)
        .with_context(|| format!("feature expression {expression:?}"))?;
    let connection = state.database.lock().expect("database lock poisoned");
    let preset = save_feature_preset(
        &connection,
        &request.name,
        expression,
        &request.note,
        request.accepted,
    )?;
    Ok((StatusCode::CREATED, Json(preset)))
}

async fn promote_feature_preset(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
    Json(request): Json<PromoteFeatureRequest>,
) -> Result<Json<FeaturePresetRecord>, ApiError> {
    let connection = state.database.lock().expect("database lock poisoned");
    Ok(Json(set_feature_promoted(
        &connection,
        &id,
        request.accepted,
    )?))
}

async fn remove_feature_preset(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Result<StatusCode, ApiError> {
    let connection = state.database.lock().expect("database lock poisoned");
    delete_feature_preset(&connection, &id)?;
    Ok(StatusCode::NO_CONTENT)
}

/// The study's accepted features and targets (`accepted.parquet`) as a download.
async fn study_export(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Result<axum::response::Response, ApiError> {
    let study = {
        let connection = state.database.lock().expect("database lock poisoned");
        query_studies(&connection, 1000)?
            .into_iter()
            .find(|study| study.id == id)
            .context("unknown study")?
    };
    let dir = checked_artifact_path(&state.root, &study.artifact_dir)?;
    let bytes = tokio::fs::read(dir.join("accepted.parquet"))
        .await
        .context("this study has no accepted-feature export; run it with an accepted set")?;
    let disposition = format!("attachment; filename=\"{}-accepted.parquet\"", study.id);
    Ok((
        [
            (
                axum::http::header::CONTENT_TYPE,
                "application/vnd.apache.parquet".to_owned(),
            ),
            (axum::http::header::CONTENT_DISPOSITION, disposition),
        ],
        bytes,
    )
        .into_response())
}

fn insert_study(state: &AppState, record: &StudyRecord) -> Result<()> {
    {
        let connection = state.database.lock().expect("database lock poisoned");
        connection.execute(
            "INSERT INTO studies (id, name, status, start_date, end_date, config_json, created_at, artifact_dir)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                record.id,
                record.name,
                record.status,
                record.start_date,
                record.end_date,
                record.config_json,
                record.created_at,
                record.artifact_dir
            ],
        )?;
    }
    Ok(())
}

fn spawn_study(
    state: AppState,
    id: String,
    artifact_dir: String,
    config: tessera::study::StudyConfig,
    start: NaiveDate,
    end: NaiveDate,
) {
    let worker_state = state;
    let worker_config = config;
    tokio::spawn(async move {
        let output_dir = worker_state.root.join(&artifact_dir);
        let result = tokio::task::spawn_blocking(move || {
            fs::create_dir_all(&output_dir)?;
            let config_path = output_dir.join("study_config.toml");
            fs::write(&config_path, toml::to_string_pretty(&worker_config)?)?;
            tessera::study::run(&worker_config, start, end, &output_dir).map(|_| ())
        })
        .await
        .unwrap_or_else(|error| Err(anyhow::anyhow!("study task failed: {error}")));
        let (status, error) = match result {
            Ok(()) => ("complete", None),
            Err(error) => ("failed", Some(format!("{error:#}"))),
        };
        let connection = worker_state
            .database
            .lock()
            .expect("database lock poisoned");
        let _ = connection.execute(
            "UPDATE studies SET status=?2, finished_at=?3, error=?4 WHERE id=?1",
            params![id, status, Utc::now().to_rfc3339(), error],
        );
    });
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
    // Archived rows (pre-SDK strategies kept for run history) stay out of the headline counts.
    let production_strategies = strategies
        .iter()
        .filter(|item| item.status == "Production")
        .count();
    let archived_strategies = strategies
        .iter()
        .filter(|item| item.status == "Archived")
        .count();
    Ok(DashboardResponse {
        strategies,
        recent_runs,
        jobs,
        production_strategies,
        archived_strategies,
        historical_reports,
        active_jobs,
        worker_capacity: 2,
    })
}

/// The catalog row: the strategy's own columns, its run count, and its newest completed run
/// joined in (`l`), so one statement serves both readers below. Append the WHERE clause.
const STRATEGY_ROW_SQL: &str =
    "SELECT s.id, s.name, s.version, s.status, s.description, s.asset_scope, s.config_path,
            s.runnable, s.base_strategy_id, s.source_sha256, s.sdk_strategy_id,
            (SELECT COUNT(*) FROM runs r WHERE r.strategy_id = s.id),
            l.id, l.created_at, l.metrics_json
     FROM strategies s
     LEFT JOIN runs l ON l.id = (
         SELECT r.id FROM runs r
         WHERE r.strategy_id = s.id AND r.status = 'Complete'
         ORDER BY r.created_at DESC, r.id DESC
         LIMIT 1)";

fn strategy_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<StrategyRecord> {
    let last_run = match (
        row.get::<_, Option<String>>(12)?,
        row.get::<_, Option<String>>(13)?,
    ) {
        (Some(run_id), Some(created_at)) => Some(StrategyLastRun {
            run_id,
            created_at,
            metrics: parse_metrics_json(row.get::<_, Option<String>>(14)?),
        }),
        _ => None,
    };
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
        run_count: row.get::<_, i64>(11)?.max(0) as usize,
        last_run,
    })
}

fn query_strategies(connection: &Connection) -> Result<Vec<StrategyRecord>> {
    let mut statement = connection.prepare(&format!(
        "{STRATEGY_ROW_SQL} WHERE s.id != 'sdk' ORDER BY s.name"
    ))?;
    let rows = statement.query_map([], strategy_from_row)?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

fn query_strategy(connection: &Connection, id: &str) -> Result<StrategyRecord> {
    Ok(connection.query_row(
        &format!("{STRATEGY_ROW_SQL} WHERE s.id = ?1"),
        [id],
        strategy_from_row,
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
    // `run-strategy` writes its own report; rebuilding it here reloaded a universe-sized
    // coverage table a second time. Only build when the simulation left none behind.
    if needs_standard_report
        && outputs.iter().all(|(_, output)| output.status.success())
        && !report_path.is_file()
    {
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

/// An API failure: the status to answer with and the error. Any error converts to a 400;
/// `api_error` names another status (404, 409, 422, 502).
#[derive(Debug)]
struct ApiError(StatusCode, anyhow::Error);

impl<E> From<E> for ApiError
where
    E: Into<anyhow::Error>,
{
    fn from(error: E) -> Self {
        Self(StatusCode::BAD_REQUEST, error.into())
    }
}

fn api_error(status: StatusCode, message: impl Into<String>) -> ApiError {
    ApiError(status, anyhow::anyhow!(message.into()))
}

impl axum::response::IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        (
            self.0,
            Json(serde_json::json!({ "error": format!("{:#}", self.1) })),
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

    /// WB-13: the studies form lists every base beyond the bar: the declared series with their
    /// kind and availability rule, and the four lake side feeds, flagged as lake-only.
    #[test]
    fn study_series_catalog_lists_declared_series_and_lake_feeds() {
        let declared: Vec<tessera::series::SeriesSpec> =
            toml::from_str::<std::collections::BTreeMap<String, Vec<tessera::series::SeriesSpec>>>(
                r#"
            [[series]]
            name = "vix"
            path = "series/vix.csv"
            publication_lag_secs = 3600

            [[series]]
            name = "fomc"
            path = "series/fomc.parquet"
            kind = "event"
            available_at_column = "released_at"
            "#,
            )
            .unwrap()
            .remove("series")
            .unwrap();
        let catalog = study_series_catalog(&declared);
        let names: Vec<&str> = catalog.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "vix",
                "fomc",
                "funding_rate",
                "funding_annualized",
                "open_interest",
                "open_interest_usd"
            ]
        );
        let vix = &catalog[0];
        assert_eq!(vix.kind, "level");
        assert!(!vix.lake_only);
        assert!(vix.source.contains("vix.csv"), "{}", vix.source);
        assert!(vix.availability.contains("3600"), "{}", vix.availability);
        let fomc = &catalog[1];
        assert_eq!(fomc.kind, "event");
        assert!(
            fomc.availability.contains("released_at"),
            "{}",
            fomc.availability
        );
        for feed in &catalog[2..] {
            assert_eq!(feed.kind, "lake");
            assert!(feed.lake_only);
            assert!(
                feed.availability.contains("receipt"),
                "{}",
                feed.availability
            );
            assert!(feed.source.contains("funding") || feed.source.contains("open_interest"));
        }
        // With nothing declared the feeds alone remain.
        assert_eq!(study_series_catalog(&[]).len(), 4);
    }

    /// WB-09: a feature preset lives in the catalog database, so it is still there after the
    /// service restarts (a fresh connection to the same file), and its promotion with it.
    #[test]
    fn feature_presets_survive_reopening_the_catalog() {
        let dir = std::env::temp_dir().join(format!("tessera-wb09-catalog-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("tessera_ui.sqlite3");
        let preset = {
            let connection = Connection::open(&path).unwrap();
            migrate(&connection).unwrap();
            let preset =
                save_feature_preset(&connection, "spread z", "spread_bps | zscore 60", "", false)
                    .unwrap();
            assert!(!preset.accepted && preset.promoted_at.is_none());
            assert!(promoted_expressions(&connection).unwrap().is_empty());
            set_feature_promoted(&connection, &preset.id, true).unwrap()
        };
        assert!(preset.accepted && preset.promoted_at.is_some());
        // The service restarts: a new connection, the same file, the same migration.
        let connection = Connection::open(&path).unwrap();
        migrate(&connection).unwrap();
        let presets = query_feature_presets(&connection).unwrap();
        assert_eq!(presets.len(), 1);
        assert_eq!(presets[0].id, preset.id);
        assert_eq!(presets[0].name, "spread z");
        assert_eq!(presets[0].expression, "spread_bps | zscore 60");
        assert!(presets[0].accepted && presets[0].promoted_at.is_some());
        assert_eq!(
            promoted_expressions(&connection).unwrap(),
            vec!["spread_bps | zscore 60"]
        );
        // Demotion takes it out of the accepted set; deletion removes it.
        let demoted = set_feature_promoted(&connection, &preset.id, false).unwrap();
        assert!(!demoted.accepted && demoted.promoted_at.is_none());
        assert!(promoted_expressions(&connection).unwrap().is_empty());
        delete_feature_preset(&connection, &preset.id).unwrap();
        assert!(query_feature_presets(&connection).unwrap().is_empty());
        drop(connection);
        let _ = fs::remove_dir_all(&dir);
    }

    /// WB-15: a fresh catalog starts with the vol feature set; a second start adds nothing,
    /// and an edit to a seeded preset survives it.
    #[test]
    fn a_fresh_catalog_is_seeded_with_the_vol_features_once() {
        let connection = Connection::open_in_memory().unwrap();
        migrate(&connection).unwrap();
        assert!(query_feature_presets(&connection).unwrap().is_empty());
        seed_feature_presets(&connection).unwrap();
        let presets = query_feature_presets(&connection).unwrap();
        assert_eq!(presets.len(), SEEDED_FEATURE_PRESETS.len());
        let mut names: Vec<&str> = presets.iter().map(|p| p.name.as_str()).collect();
        names.sort_unstable();
        assert_eq!(
            names,
            vec![
                "rv 30s",
                "rv 5s",
                "rv 60s",
                "vol of vol 60s",
                "vol ratio 5s/60s"
            ]
        );
        let ratio = presets
            .iter()
            .find(|p| p.name == "vol ratio 5s/60s")
            .unwrap();
        assert_eq!(ratio.expression, "mid | rv 5s | ratio_to rv 60s");
        assert!(!ratio.accepted);
        // Every seeded expression parses under the grammar.
        for (_, expression, _) in SEEDED_FEATURE_PRESETS {
            tessera::feature_expr::parse(expression)
                .unwrap_or_else(|e| panic!("{expression}: {e}"));
        }
        // The owner edits one and deletes another; the next start touches neither.
        save_feature_preset(&connection, "rv 5s", "mid | rv 10s", "edited", false).unwrap();
        let sixty = presets.iter().find(|p| p.name == "rv 60s").unwrap();
        delete_feature_preset(&connection, &sixty.id).unwrap();
        seed_feature_presets(&connection).unwrap();
        let again = query_feature_presets(&connection).unwrap();
        assert_eq!(
            again.len(),
            SEEDED_FEATURE_PRESETS.len(),
            "the deleted preset comes back once, nothing duplicates"
        );
        let five = again.iter().find(|p| p.name == "rv 5s").unwrap();
        assert_eq!(five.expression, "mid | rv 10s", "an edit survives the seed");
        seed_feature_presets(&connection).unwrap();
        assert_eq!(
            query_feature_presets(&connection).unwrap().len(),
            SEEDED_FEATURE_PRESETS.len()
        );
    }

    /// WB-09: promoting a preset changes what every study regresses candidates against: the
    /// accepted set is the request's own expressions plus every promoted preset, once each.
    #[test]
    fn promoted_features_join_the_accepted_set() {
        let connection = Connection::open_in_memory().unwrap();
        migrate(&connection).unwrap();
        let obi = save_feature_preset(&connection, "obi", "obi_l1", "", true).unwrap();
        save_feature_preset(&connection, "spread", "spread_bps", "candidate", false).unwrap();
        assert_eq!(promoted_expressions(&connection).unwrap(), vec!["obi_l1"]);
        let promoted = promoted_expressions(&connection).unwrap();
        assert_eq!(
            merge_accepted(&["spread_bps".to_owned(), " obi_l1 ".to_owned()], &promoted),
            vec!["spread_bps", "obi_l1"]
        );
        assert_eq!(merge_accepted(&[], &promoted), vec!["obi_l1"]);
        assert!(merge_accepted(&[" ".to_owned()], &[]).is_empty());
        // Saving under an existing name updates the expression and keeps the promotion, so a
        // promoted feature can be refined without falling out of the accepted set.
        let again = save_feature_preset(&connection, "obi", "obi_l5", "", false).unwrap();
        assert_eq!(again.id, obi.id);
        assert!(again.accepted);
        assert_eq!(promoted_expressions(&connection).unwrap(), vec!["obi_l5"]);
        assert_eq!(query_feature_presets(&connection).unwrap().len(), 2);
        set_feature_promoted(&connection, &obi.id, false).unwrap();
        assert!(promoted_expressions(&connection).unwrap().is_empty());
        // Names are trimmed and bounded; a blank expression is refused.
        assert!(save_feature_preset(&connection, "  ", "obi_l1", "", false).is_err());
        assert!(save_feature_preset(&connection, "x", "   ", "", false).is_err());
    }

    /// UI-06: each catalog row carries its last completed run (date and cached metrics) so the
    /// strategies page is a scoreboard. A strategy with two completed runs reports the newer
    /// one's metrics, not the older's and not a later failed run; one whose only completed run
    /// has no cached metrics reports the date with null metrics; one with no runs reports null.
    #[test]
    fn catalog_rows_carry_the_last_completed_runs_metrics() {
        let connection = Connection::open_in_memory().unwrap();
        migrate(&connection).unwrap();
        for id in ["gap_fade", "limit_buyer", "orb_breakout"] {
            connection
                .execute(
                    "INSERT INTO strategies
                     (id, name, version, status, description, asset_scope, config_path,
                      runnable, created_at, base_strategy_id)
                     VALUES (?1, ?1, 'v1', 'Research', '', 'ETF', 'x.toml', 1,
                             '2026-09-01T00:00:00Z', 'sdk')",
                    [id],
                )
                .unwrap();
        }
        let older = RunMetrics {
            cagr_percent: Some(4.0),
            total_return_percent: Some(9.0),
            sharpe: Some(0.5),
            sortino: None,
            calmar: None,
            max_drawdown_percent: Some(20.0),
            annual_volatility_percent: None,
            win_rate_percent: None,
            trades: Some(10),
            start: None,
            end: None,
        };
        let newer = RunMetrics {
            cagr_percent: Some(18.2),
            sharpe: Some(1.31),
            max_drawdown_percent: Some(9.4),
            trades: Some(42),
            ..older.clone()
        };
        let runs = [
            (
                "gap_fade-1",
                "gap_fade",
                "Complete",
                "2026-09-08T00:00:00Z",
                Some(&older),
            ),
            (
                "gap_fade-2",
                "gap_fade",
                "Complete",
                "2026-09-10T00:00:00Z",
                Some(&newer),
            ),
            // A later run that did not complete never becomes the strategy's headline.
            (
                "gap_fade-3",
                "gap_fade",
                "Failed",
                "2026-09-11T00:00:00Z",
                None,
            ),
            (
                "limit_buyer-1",
                "limit_buyer",
                "Complete",
                "2026-09-09T00:00:00Z",
                None,
            ),
        ];
        for (id, strategy, status, created_at, metrics) in runs {
            connection
                .execute(
                    "INSERT INTO runs
                     (id, strategy_id, name, research_label, status, artifact_dir, created_at,
                      metrics_json)
                     VALUES (?1, ?2, ?1, 'baseline', ?3, ?1, ?4, ?5)",
                    params![
                        id,
                        strategy,
                        status,
                        created_at,
                        metrics.map(|m| serde_json::to_string(m).unwrap())
                    ],
                )
                .unwrap();
        }

        let rows = query_strategies(&connection).unwrap();
        let row = |id: &str| rows.iter().find(|row| row.id == id).unwrap();

        let gap_fade = row("gap_fade");
        assert_eq!(gap_fade.run_count, 3);
        let last = gap_fade.last_run.as_ref().expect("a completed run");
        assert_eq!(last.run_id, "gap_fade-2");
        assert_eq!(last.created_at, "2026-09-10T00:00:00Z");
        let metrics = last.metrics.as_ref().expect("cached metrics");
        assert_eq!(metrics.cagr_percent, Some(18.2));
        assert_eq!(metrics.sharpe, Some(1.31));
        assert_eq!(metrics.max_drawdown_percent, Some(9.4));

        let limit_buyer = row("limit_buyer");
        assert_eq!(limit_buyer.run_count, 1);
        let last = limit_buyer.last_run.as_ref().expect("a completed run");
        assert_eq!(last.run_id, "limit_buyer-1");
        assert_eq!(last.created_at, "2026-09-09T00:00:00Z");
        assert!(
            last.metrics.is_none(),
            "no cached metrics gives null metrics"
        );

        let orb_breakout = row("orb_breakout");
        assert_eq!(orb_breakout.run_count, 0);
        assert!(
            orb_breakout.last_run.is_none(),
            "no run gives a null last run"
        );

        // The single-strategy reader agrees with the catalog, and the wire shape carries the
        // nulls the page renders as a dash.
        let one = query_strategy(&connection, "gap_fade").unwrap();
        assert_eq!(
            one.last_run.as_ref().map(|l| l.run_id.as_str()),
            Some("gap_fade-2")
        );
        let json = serde_json::to_value(&rows).unwrap();
        let by_id = |id: &str| {
            json.as_array()
                .unwrap()
                .iter()
                .find(|row| row["id"] == id)
                .unwrap()
                .clone()
        };
        assert!(by_id("orb_breakout")["last_run"].is_null());
        assert!(by_id("limit_buyer")["last_run"]["metrics"].is_null());
        assert_eq!(by_id("gap_fade")["last_run"]["metrics"]["sharpe"], 1.31);
    }

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

    /// DS-03 (decisions 0020, 0021): sources are registered through the service against the
    /// DS-02 stub, the token lives in a 0600 file under a 0700 folder, and neither the token
    /// nor its path appears in any `/api/sources` or `/api/data` response.
    mod sources {
        use super::*;
        use std::os::unix::fs::PermissionsExt;
        use std::sync::atomic::{AtomicBool, Ordering};

        use axum::extract::Query as AxumQuery;
        use axum::response::Response;

        /// Placeholders, never real tokens. The stub accepts exactly these two.
        const TOKEN: &str = "stub-token-0000";
        const REPLACEMENT: &str = "stub-token-1111";

        #[derive(Clone)]
        struct Stub {
            down: Arc<AtomicBool>,
        }

        /// The stub's refusals before any fixture is served: 503 while it is down, 401 for a
        /// token it does not know.
        fn gate(stub: &Stub, query: &[(String, String)]) -> Option<Response> {
            if stub.down.load(Ordering::SeqCst) {
                return Some(
                    (StatusCode::SERVICE_UNAVAILABLE, "Service Unavailable").into_response(),
                );
            }
            let token = query
                .iter()
                .find(|(k, _)| k == "api_token")
                .map(|(_, v)| v.as_str());
            if token != Some(TOKEN) && token != Some(REPLACEMENT) {
                return Some(
                    (
                        StatusCode::UNAUTHORIZED,
                        [("content-type", "application/json")],
                        r#"{"message":"Unauthenticated","code":401}"#,
                    )
                        .into_response(),
                );
            }
            None
        }

        fn fixture(name: &str) -> String {
            fs::read_to_string(format!(
                "{}/tests/fixtures/eodhd/{name}.json",
                env!("CARGO_MANIFEST_DIR")
            ))
            .unwrap()
        }

        fn json_response(status: StatusCode, body: String) -> Response {
            (status, [("content-type", "application/json")], body).into_response()
        }

        async fn stub_user(
            State(stub): State<Stub>,
            AxumQuery(query): AxumQuery<Vec<(String, String)>>,
        ) -> Response {
            if let Some(refused) = gate(&stub, &query) {
                return refused;
            }
            json_response(StatusCode::OK, fixture("user"))
        }

        async fn stub_exchanges(
            State(stub): State<Stub>,
            AxumQuery(query): AxumQuery<Vec<(String, String)>>,
        ) -> Response {
            if let Some(refused) = gate(&stub, &query) {
                return refused;
            }
            json_response(StatusCode::OK, fixture("exchanges-list"))
        }

        /// US answers DS-02's recorded list, and two delisted rows with `delisted=1`; LSE two
        /// rows of its own and no delisted ones; anything else the provider's 404.
        async fn stub_symbols(
            State(stub): State<Stub>,
            AxumPath(exchange): AxumPath<String>,
            AxumQuery(query): AxumQuery<Vec<(String, String)>>,
        ) -> Response {
            if let Some(refused) = gate(&stub, &query) {
                return refused;
            }
            let delisted = query.iter().any(|(k, v)| k == "delisted" && v == "1");
            match (exchange.as_str(), delisted) {
                ("US", false) => json_response(StatusCode::OK, fixture("exchange-symbol-list-US")),
                ("US", true) => json_response(
                    StatusCode::OK,
                    r#"[{"Code":"YHOO","Name":"Yahoo Inc","Type":"Common Stock","Currency":"USD"},
                        {"Code":"TWTR","Name":"Twitter Inc","Type":"Common Stock","Currency":"USD"}]"#
                        .to_owned(),
                ),
                ("LSE", false) => json_response(
                    StatusCode::OK,
                    r#"[{"Code":"VOD","Name":"Vodafone Group","Type":"Common Stock","Currency":"GBP"},
                        {"Code":"ISF","Name":"iShares Core FTSE 100","Type":"ETF","Currency":"GBP"}]"#
                        .to_owned(),
                ),
                ("LSE", true) => json_response(StatusCode::OK, "[]".to_owned()),
                _ => json_response(
                    StatusCode::NOT_FOUND,
                    r#"{"message":"Unknown exchange"}"#.to_owned(),
                ),
            }
        }

        async fn serve(router: Router) -> String {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
            format!("http://{addr}")
        }

        /// The DS-02 stub's `/api/user`, `/api/exchanges-list/`, and
        /// `/api/exchange-symbol-list/{code}`, with a switch that makes every route answer 503.
        async fn stub() -> (String, Arc<AtomicBool>) {
            let down = Arc::new(AtomicBool::new(false));
            let router = Router::new()
                .route("/api/user", get(stub_user))
                .route("/api/exchanges-list/", get(stub_exchanges))
                .route("/api/exchange-symbol-list/{exchange}", get(stub_symbols))
                .with_state(Stub { down: down.clone() });
            (serve(router).await, down)
        }

        /// A service over an in-memory catalog, a scratch root, and the stub as EODHD.
        fn test_state(root: &Path, eodhd_base_url: &str) -> AppState {
            let connection = Connection::open_in_memory().unwrap();
            migrate(&connection).unwrap();
            AppState {
                root: root.to_path_buf(),
                local: Arc::new(LocalConfig::bundled_example(Path::new(env!(
                    "CARGO_MANIFEST_DIR"
                )))),
                database: Arc::new(Mutex::new(connection)),
                workers: Arc::new(Semaphore::new(2)),
                instruments: Arc::new(Mutex::new(None)),
                sdk_manifests: Arc::new(Mutex::new(std::collections::HashMap::new())),
                data_sources: Arc::new(Mutex::new(None)),
                eodhd_base_url: Arc::new(eodhd_base_url.to_owned()),
                scans: Arc::new(Mutex::new(std::collections::HashSet::new())),
                dataset_jobs: Arc::new(Mutex::new(std::collections::HashMap::new())),
            }
        }

        fn source_count(state: &AppState) -> i64 {
            state
                .database
                .lock()
                .unwrap()
                .query_row("SELECT COUNT(*) FROM data_sources", [], |row| row.get(0))
                .unwrap()
        }

        fn mode_of(path: &Path) -> u32 {
            fs::metadata(path).unwrap().permissions().mode() & 0o777
        }

        fn scratch_root(tag: &str) -> PathBuf {
            std::env::temp_dir().join(format!(
                "tessera-ds03-{tag}-{}-{}",
                std::process::id(),
                Utc::now().timestamp_nanos_opt().unwrap_or(0)
            ))
        }

        #[tokio::test]
        async fn a_registered_sources_token_and_file_stay_out_of_every_response() {
            let (eodhd, down) = stub().await;
            let root = scratch_root("service");
            let library = root.join("library");
            fs::create_dir_all(&library).unwrap();
            let state = test_state(&root, &eodhd);
            let api = serve(api_router().with_state(state.clone())).await;
            let client = reqwest::Client::new();
            let secrets_dir = root.join("data/ui/secrets");
            let secrets_path = secrets_dir.display().to_string();
            let leaks = [TOKEN, REPLACEMENT, secrets_path.as_str(), "data/ui/secrets"];
            let mut responses: Vec<(String, String)> = Vec::new();

            // A rejected token is refused with the provider's message: no row, no file.
            let response = client
                .post(format!("{api}/api/sources"))
                .json(&serde_json::json!({
                    "kind": "eodhd", "name": "EODHD", "root": library,
                    "catalog_dir": library.join("catalog"), "token": "wrong-token-9999"
                }))
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
            let body = response.text().await.unwrap();
            assert!(body.contains("Unauthenticated"), "{body}");
            responses.push(("POST rejected".into(), body));
            assert_eq!(source_count(&state), 0);
            assert!(
                !secrets_dir.exists() || fs::read_dir(&secrets_dir).unwrap().next().is_none(),
                "a rejected token left a file"
            );

            // A kind that is not compiled in is refused before any call.
            let response = client
                .post(format!("{api}/api/sources"))
                .json(&serde_json::json!({
                    "kind": "quandl", "name": "Q", "root": library,
                    "catalog_dir": library.join("catalog"), "token": TOKEN
                }))
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::BAD_REQUEST);
            responses.push(("POST unknown kind".into(), response.text().await.unwrap()));
            assert_eq!(source_count(&state), 0);

            // A verified token registers the source: a row, a 0600 file in a 0700 folder.
            let response = client
                .post(format!("{api}/api/sources"))
                .json(&serde_json::json!({
                    "kind": "eodhd", "name": "EODHD", "root": library,
                    "catalog_dir": library.join("catalog"), "token": TOKEN
                }))
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::CREATED);
            let body = response.text().await.unwrap();
            let card: serde_json::Value = serde_json::from_str(&body).unwrap();
            responses.push(("POST created".into(), body));
            let id = card["id"].as_str().unwrap().to_owned();
            assert_eq!(card["kind"], "eodhd");
            assert_eq!(card["verify_state"], "connected");
            assert_eq!(card["token_set"], true);
            assert!(card["token_set_at"].is_string() && card["verified_at"].is_string());
            assert_eq!(card["root_exists"], true);
            assert_eq!(card["reserve_pct"], 5.0);
            let volume = &card["volume"];
            assert!(volume["total_bytes"].as_u64().unwrap() > 0, "{volume}");
            assert!(
                volume["used_bytes"].as_u64().unwrap() + volume["free_bytes"].as_u64().unwrap()
                    <= volume["total_bytes"].as_u64().unwrap(),
                "{volume}"
            );
            assert_eq!(source_count(&state), 1);
            let token_file = secrets_dir.join(format!("{id}.token"));
            assert_eq!(fs::read_to_string(&token_file).unwrap(), TOKEN);
            assert_eq!(mode_of(&token_file), 0o600);
            assert_eq!(mode_of(&secrets_dir), 0o700);

            // A rejected replacement changes nothing; an accepted one rewrites the file 0600.
            let response = client
                .put(format!("{api}/api/sources/{id}/token"))
                .json(&serde_json::json!({ "token": "wrong-token-9999" }))
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
            responses.push(("PUT token rejected".into(), response.text().await.unwrap()));
            assert_eq!(fs::read_to_string(&token_file).unwrap(), TOKEN);
            let response = client
                .put(format!("{api}/api/sources/{id}/token"))
                .json(&serde_json::json!({ "token": REPLACEMENT }))
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let body = response.text().await.unwrap();
            let card: serde_json::Value = serde_json::from_str(&body).unwrap();
            responses.push(("PUT token".into(), body));
            assert_eq!(card["verify_state"], "connected");
            assert_eq!(fs::read_to_string(&token_file).unwrap(), REPLACEMENT);
            assert_eq!(mode_of(&token_file), 0o600);
            assert_eq!(
                fs::read_dir(&secrets_dir).unwrap().count(),
                1,
                "the replacement left a part file behind"
            );

            // Re-verifying with the provider down records Unreachable and keeps the source.
            down.store(true, Ordering::SeqCst);
            let response = client
                .post(format!("{api}/api/sources/{id}/verify"))
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let body = response.text().await.unwrap();
            let card: serde_json::Value = serde_json::from_str(&body).unwrap();
            responses.push(("POST verify down".into(), body));
            assert_eq!(card["verify_state"], "unreachable");
            assert!(
                card["verify_message"].as_str().unwrap().contains("503"),
                "{card}"
            );
            down.store(false, Ordering::SeqCst);
            let response = client
                .post(format!("{api}/api/sources/{id}/verify"))
                .send()
                .await
                .unwrap();
            let body = response.text().await.unwrap();
            let card: serde_json::Value = serde_json::from_str(&body).unwrap();
            responses.push(("POST verify up".into(), body));
            assert_eq!(card["verify_state"], "connected");
            assert!(card["verify_message"].is_null(), "{card}");

            // The list carries the card and the kinds compiled in.
            let body = client
                .get(format!("{api}/api/sources"))
                .send()
                .await
                .unwrap()
                .text()
                .await
                .unwrap();
            let list: serde_json::Value = serde_json::from_str(&body).unwrap();
            responses.push(("GET sources".into(), body));
            assert_eq!(list["kinds"], serde_json::json!(["eodhd"]));
            assert_eq!(list["sources"].as_array().unwrap().len(), 1);
            assert_eq!(list["sources"][0]["id"], id.as_str());
            assert_eq!(list["sources"][0]["name"], "EODHD");
            assert_eq!(
                list["sources"][0]["root"],
                library.display().to_string().as_str()
            );

            // The Data page's other responses stay clean as well.
            for path in ["/api/data/sources?refresh=1", "/api/data/status"] {
                let response = client.get(format!("{api}{path}")).send().await.unwrap();
                assert_eq!(response.status(), StatusCode::OK, "{path}");
                responses.push((format!("GET {path}"), response.text().await.unwrap()));
            }

            // A file under the root refuses the delete; without one, row and file go.
            fs::create_dir_all(library.join("eod")).unwrap();
            fs::write(library.join("eod/SPY.US.csv"), "Date,Close\n2026-01-02,1\n").unwrap();
            let response = client
                .delete(format!("{api}/api/sources/{id}"))
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::CONFLICT);
            responses.push(("DELETE refused".into(), response.text().await.unwrap()));
            assert_eq!(source_count(&state), 1);
            assert!(token_file.is_file());
            fs::remove_dir_all(library.join("eod")).unwrap();
            let response = client
                .delete(format!("{api}/api/sources/{id}"))
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::NO_CONTENT);
            assert_eq!(source_count(&state), 0);
            assert!(!token_file.exists(), "the token file outlived its source");
            assert!(
                library.is_dir(),
                "the console must never remove data folders"
            );
            let response = client
                .delete(format!("{api}/api/sources/{id}"))
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::NOT_FOUND);
            responses.push(("DELETE missing".into(), response.text().await.unwrap()));

            for (label, body) in &responses {
                for leak in &leaks {
                    assert!(!body.contains(leak), "{label} carries {leak:?}: {body}");
                }
            }
            let _ = fs::remove_dir_all(&root);
        }

        fn table_count(state: &AppState, table: &str) -> i64 {
            state
                .database
                .lock()
                .unwrap()
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .unwrap()
        }

        /// Sends `request` and returns the status, the body parsed as JSON (Null when it is
        /// not), and the body's text for the leak check.
        async fn call(request: reqwest::RequestBuilder) -> (StatusCode, serde_json::Value, String) {
            let response = request.send().await.unwrap();
            let status = response.status();
            let text = response.text().await.unwrap();
            let value = serde_json::from_str(&text).unwrap_or(serde_json::Value::Null);
            (status, value, text)
        }

        /// DS-04 (decisions 0013, 0020): a source's exchange list and listings are cached in
        /// the catalog with the time fetched and served with per-type counts; a refresh that
        /// fails keeps the rows and their time and sets the unreachable note; a refresh with no
        /// exchange named fetches the listings of the exchanges with datasets; the cache goes
        /// with its source.
        #[tokio::test]
        async fn the_providers_availability_is_cached_and_an_outage_keeps_the_rows() {
            let (eodhd, down) = stub().await;
            let root = scratch_root("availability");
            let library = root.join("library");
            fs::create_dir_all(&library).unwrap();
            let state = test_state(&root, &eodhd);
            let api = serve(api_router().with_state(state.clone())).await;
            let client = reqwest::Client::new();
            let secrets_path = root.join("data/ui/secrets").display().to_string();
            let leaks = [TOKEN, secrets_path.as_str(), "data/ui/secrets"];
            let mut responses: Vec<(String, String)> = Vec::new();

            let (status, card, text) = call(client.post(format!("{api}/api/sources")).json(
                &serde_json::json!({
                    "kind": "eodhd", "name": "EODHD", "root": library,
                    "catalog_dir": library.join("catalog"), "token": TOKEN
                }),
            ))
            .await;
            assert_eq!(status, StatusCode::CREATED, "{text}");
            responses.push(("POST source".into(), text));
            let id = card["id"].as_str().unwrap().to_owned();
            let availability = format!("{api}/api/sources/{id}/availability");

            // Before any refresh: an empty table, no time, no note.
            let (status, body, text) = call(client.get(&availability)).await;
            assert_eq!(status, StatusCode::OK, "{text}");
            responses.push(("GET empty".into(), text));
            assert_eq!(body["source_id"], id.as_str());
            assert_eq!(body["exchanges"], serde_json::json!([]));
            assert!(
                body["fetched_at"].is_null() && body["refreshed_at"].is_null(),
                "{body}"
            );
            assert!(body["unreachable"].is_null(), "{body}");
            let (status, _, text) =
                call(client.get(format!("{api}/api/sources/nope/availability"))).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{text}");

            // A refresh naming US caches the four exchanges and the US listing.
            let (status, first, text) = call(
                client
                    .post(format!("{availability}/refresh"))
                    .json(&serde_json::json!({ "exchange": "US" })),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{text}");
            responses.push(("POST refresh US".into(), text));
            assert!(first["unreachable"].is_null(), "{first}");
            let fetched_at = first["fetched_at"].as_str().unwrap().to_owned();
            assert!(first["refreshed_at"].is_string(), "{first}");
            let exchanges = first["exchanges"].as_array().unwrap();
            let codes: Vec<&str> = exchanges
                .iter()
                .map(|e| e["code"].as_str().unwrap())
                .collect();
            assert_eq!(codes, ["CC", "LSE", "TO", "US"]);
            let us = exchanges.iter().find(|e| e["code"] == "US").unwrap();
            assert_eq!(us["name"], "USA Stocks");
            assert_eq!(us["country"], "USA");
            assert_eq!(
                us["resolutions"],
                serde_json::json!(["daily", "1h", "5m", "1m"])
            );
            assert_eq!(us["fetched_at"], fetched_at.as_str());
            assert_eq!(us["listed"], 3);
            assert_eq!(
                us["types"],
                serde_json::json!([
                    { "type": "Common Stock", "count": 2 },
                    { "type": "ETF", "count": 1 }
                ])
            );
            let us_listed_at = us["listings_fetched_at"].as_str().unwrap().to_owned();
            assert_eq!(us["delisted"], 0, "{us}");
            assert!(us["delisted_fetched_at"].is_null(), "{us}");
            let lse = exchanges.iter().find(|e| e["code"] == "LSE").unwrap();
            assert_eq!(lse["country"], "UK");
            assert_eq!(lse["resolutions"], serde_json::json!(["daily", "1h", "5m"]));
            assert_eq!(lse["listed"], 0);
            assert_eq!(lse["types"], serde_json::json!([]));
            assert!(lse["listings_fetched_at"].is_null(), "{lse}");
            assert_eq!(table_count(&state, "provider_exchanges"), 4);
            assert_eq!(table_count(&state, "provider_listings"), 3);

            // The read serves exactly what the refresh answered.
            let (status, body, text) = call(client.get(&availability)).await;
            assert_eq!(status, StatusCode::OK, "{text}");
            responses.push(("GET cached".into(), text));
            assert_eq!(body, first);

            // An exchange the provider does not list is refused; the cache is untouched.
            let (status, _, text) = call(
                client
                    .post(format!("{availability}/refresh"))
                    .json(&serde_json::json!({ "exchange": "MARS" })),
            )
            .await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{text}");
            assert!(text.contains("MARS"), "{text}");
            responses.push(("POST refresh unknown".into(), text));
            let (_, body, _) = call(client.get(&availability)).await;
            assert_eq!(body, first);

            // With the provider down, a refresh keeps the rows and their time and sets the note.
            down.store(true, Ordering::SeqCst);
            let (status, body, text) = call(
                client
                    .post(format!("{availability}/refresh"))
                    .json(&serde_json::json!({ "exchange": "US" })),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{text}");
            responses.push(("POST refresh down".into(), text));
            assert_eq!(body["exchanges"], first["exchanges"]);
            assert_eq!(body["fetched_at"], fetched_at.as_str());
            assert_ne!(body["refreshed_at"], first["refreshed_at"]);
            let note = body["unreachable"].as_str().unwrap_or_default();
            assert!(note.contains("503"), "{body}");
            assert_eq!(table_count(&state, "provider_exchanges"), 4);
            assert_eq!(table_count(&state, "provider_listings"), 3);
            let (_, body, text) = call(client.get(&availability)).await;
            responses.push(("GET down".into(), text));
            assert_eq!(body["unreachable"], note);
            assert_eq!(body["exchanges"], first["exchanges"]);

            // Back up, a refresh with no exchange named fetches the listings of the exchanges
            // with a dataset (DS-05's table; this one excludes delisted symbols, so only the
            // active listing is asked for) and clears the note; the US listing stays as it
            // was fetched.
            down.store(false, Ordering::SeqCst);
            {
                let connection = state.database.lock().unwrap();
                connection
                    .execute(
                        "INSERT INTO datasets (id, source_id, exchange, types_json, resolution,
                                               from_date, folder, include_delisted, created_at)
                         VALUES ('d1', ?1, 'LSE', '[\"Common Stock\"]', 'daily', '2020-01-01',
                                 '/nonexistent/tessera-ds04/eod', 0, 't0')",
                        [&id],
                    )
                    .unwrap();
            }
            let (status, body, text) = call(client.post(format!("{availability}/refresh"))).await;
            assert_eq!(status, StatusCode::OK, "{text}");
            responses.push(("POST refresh datasets".into(), text));
            assert!(body["unreachable"].is_null(), "{body}");
            assert_ne!(body["fetched_at"], fetched_at.as_str());
            let exchanges = body["exchanges"].as_array().unwrap();
            let lse = exchanges.iter().find(|e| e["code"] == "LSE").unwrap();
            assert_eq!(lse["listed"], 2);
            assert_eq!(
                lse["types"],
                serde_json::json!([
                    { "type": "Common Stock", "count": 1 },
                    { "type": "ETF", "count": 1 }
                ])
            );
            assert_eq!(lse["delisted"], 0, "{lse}");
            assert!(lse["delisted_fetched_at"].is_null(), "{lse}");
            let us = exchanges.iter().find(|e| e["code"] == "US").unwrap();
            assert_eq!(us["listed"], 3);
            assert_eq!(us["listings_fetched_at"], us_listed_at.as_str());
            assert_eq!(table_count(&state, "provider_listings"), 5);

            // Without a token file the refresh is refused, as verify is.
            fs::remove_file(root.join(format!("data/ui/secrets/{id}.token"))).unwrap();
            let (status, _, text) = call(client.post(format!("{availability}/refresh"))).await;
            assert_eq!(status, StatusCode::CONFLICT, "{text}");
            responses.push(("POST refresh no token".into(), text));

            // The cache goes with its source, and so does the dataset.
            let (status, _, text) = call(client.delete(format!("{api}/api/sources/{id}"))).await;
            assert_eq!(status, StatusCode::NO_CONTENT, "{text}");
            assert_eq!(table_count(&state, "provider_exchanges"), 0);
            assert_eq!(table_count(&state, "provider_listings"), 0);
            assert_eq!(table_count(&state, "provider_refreshes"), 0);
            assert_eq!(table_count(&state, "datasets"), 0);
            let (status, _, _) = call(client.get(&availability)).await;
            assert_eq!(status, StatusCode::NOT_FOUND);

            for (label, body) in &responses {
                for leak in &leaks {
                    assert!(!body.contains(leak), "{label} carries {leak:?}: {body}");
                }
            }
            let _ = fs::remove_dir_all(&root);
        }

        /// The catalog side of DS-04 without a server: the dataset lookup names each exchange
        /// with a dataset and whether one includes delisted symbols, listings count per type
        /// largest first, and an exchange with no cached listing has no time and no types.
        #[test]
        fn the_cached_table_counts_each_exchanges_listing_by_type() {
            let mut connection = Connection::open_in_memory().unwrap();
            migrate(&connection).unwrap();
            connection
                .execute(
                    "INSERT INTO data_sources (id, name, kind, root, catalog_dir, created_at)
                     VALUES ('s1', 'EODHD', 'eodhd', '/tmp/x', '/tmp/x/catalog', 't0')",
                    [],
                )
                .unwrap();
            assert!(
                exchanges_with_datasets(&connection, "s1")
                    .unwrap()
                    .is_empty()
            );
            connection
                .execute_batch(
                    "INSERT INTO data_sources (id, name, kind, root, catalog_dir, created_at)
                     VALUES ('other', 'Other', 'eodhd', '/tmp/y', '/tmp/y/catalog', 't0');
                     INSERT INTO datasets (id, source_id, exchange, types_json, resolution,
                                           from_date, folder, include_delisted, created_at)
                     VALUES ('d1', 's1', 'US', '[]', 'daily', '2020-01-01', '/tmp/x/eod', 1, 't0'),
                            ('d2', 's1', 'LSE', '[]', 'daily', '2020-01-01', '/tmp/x/eod', 0, 't0'),
                            ('d3', 's1', 'US', '[]', '5m', '2020-01-01', '/tmp/x/5m', 0, 't0'),
                            ('d4', 'other', 'CC', '[]', 'daily', '2020-01-01', '/tmp/y/eod', 0, 't0');",
                )
                .unwrap();
            assert_eq!(
                exchanges_with_datasets(&connection, "s1").unwrap(),
                [("LSE".to_owned(), false), ("US".to_owned(), true)]
            );

            let empty = load_availability(&connection, "s1").unwrap();
            assert!(empty.exchanges.is_empty() && empty.fetched_at.is_none());
            assert!(empty.refreshed_at.is_none() && empty.unreachable.is_none());

            let exchange = |code: &str, country: &str| tessera::provider::Exchange {
                code: code.into(),
                name: format!("{code} name"),
                country: country.into(),
                resolutions: vec!["daily".into()],
            };
            let listing = |code: &str, kind: &str| tessera::provider::Listing {
                code: code.into(),
                name: String::new(),
                kind: kind.into(),
                currency: "USD".into(),
                country: String::new(),
                venue: String::new(),
            };
            store_exchanges(
                &mut connection,
                "s1",
                &[exchange("US", "USA"), exchange("LSE", "UK")],
                "t1",
            )
            .unwrap();
            store_listings(
                &mut connection,
                "s1",
                "US",
                false,
                &[
                    listing("A", "ETF"),
                    listing("B", "Common Stock"),
                    listing("C", "Common Stock"),
                    listing("D", "Fund"),
                ],
                "t2",
            )
            .unwrap();
            // A delisted listing is cached apart and not counted with the active one.
            store_listings(
                &mut connection,
                "s1",
                "US",
                true,
                &[listing("Z", "ETF")],
                "t2",
            )
            .unwrap();
            record_refresh(&connection, "s1", "t3", None).unwrap();

            let table = load_availability(&connection, "s1").unwrap();
            assert_eq!(table.fetched_at.as_deref(), Some("t1"));
            assert_eq!(table.refreshed_at.as_deref(), Some("t3"));
            assert!(table.unreachable.is_none());
            let codes: Vec<&str> = table.exchanges.iter().map(|e| e.code.as_str()).collect();
            assert_eq!(codes, ["LSE", "US"]);
            let us = &table.exchanges[1];
            assert_eq!(
                (us.listed, us.listings_fetched_at.as_deref()),
                (4, Some("t2"))
            );
            let types: Vec<(&str, u64)> = us
                .types
                .iter()
                .map(|t| (t.kind.as_str(), t.count))
                .collect();
            assert_eq!(types, [("Common Stock", 2), ("ETF", 1), ("Fund", 1)]);
            assert_eq!(
                (us.delisted, us.delisted_fetched_at.as_deref()),
                (1, Some("t2"))
            );
            let lse = &table.exchanges[0];
            assert_eq!((lse.listed, lse.listings_fetched_at.as_deref()), (0, None));
            assert!(lse.types.is_empty());
            assert_eq!(
                (lse.delisted, lse.delisted_fetched_at.as_deref()),
                (0, None)
            );

            // A second fetch of the same listing replaces it; a failed attempt only notes.
            store_listings(
                &mut connection,
                "s1",
                "US",
                false,
                &[listing("A", "ETF")],
                "t4",
            )
            .unwrap();
            record_refresh(&connection, "s1", "t5", Some("US: provider unreachable")).unwrap();
            let table = load_availability(&connection, "s1").unwrap();
            let us = &table.exchanges[1];
            assert_eq!(
                (us.listed, us.listings_fetched_at.as_deref()),
                (1, Some("t4"))
            );
            assert_eq!(table.refreshed_at.as_deref(), Some("t5"));
            assert_eq!(
                table.unreachable.as_deref(),
                Some("US: provider unreachable")
            );
            assert_eq!(table.fetched_at.as_deref(), Some("t1"));
        }

        #[test]
        fn the_root_volume_is_measured_and_a_missing_root_has_none() {
            let here = volume_figures(Path::new(env!("CARGO_MANIFEST_DIR"))).unwrap();
            assert!(here.total_bytes > 0);
            assert!(here.used_bytes <= here.total_bytes);
            assert!(here.free_bytes <= here.total_bytes);
            assert!(volume_figures(Path::new("/nonexistent/tessera-ds03")).is_none());
        }

        #[test]
        fn a_root_holds_files_only_when_a_regular_file_lies_under_it() {
            let root = scratch_root("holds");
            assert!(!holds_files(&root));
            fs::create_dir_all(root.join("eod/nested")).unwrap();
            assert!(!holds_files(&root));
            fs::write(root.join("eod/nested/x.csv"), "a").unwrap();
            assert!(holds_files(&root));
            let _ = fs::remove_dir_all(&root);
        }

        #[test]
        fn provider_errors_map_to_the_card_states_and_refusal_statuses() {
            let rejected = ProviderError::CredentialsRejected("Unauthenticated".into());
            let unreachable = ProviderError::Unreachable("HTTP 503".into());
            let malformed = ProviderError::Malformed("not json".into());
            assert_eq!(verify_state_of(&rejected).0, VERIFY_REJECTED);
            assert_eq!(verify_state_of(&unreachable).0, VERIFY_UNREACHABLE);
            assert_eq!(verify_state_of(&malformed).0, VERIFY_UNREACHABLE);
            assert_eq!(refused(rejected).0, StatusCode::UNPROCESSABLE_ENTITY);
            assert_eq!(refused(unreachable).0, StatusCode::BAD_GATEWAY);
            assert_eq!(refused(malformed).0, StatusCode::BAD_GATEWAY);
        }

        /// DS-07 (decision 0022): the card carries the usage the provider reports, refreshed
        /// by `GET /api/sources` once a minute or on `?refresh=1`; with the provider down the
        /// last figures stay with their time; `PUT /api/sources/{id}` sets the reserve.
        mod credits {
            use super::*;
            use chrono::TimeZone;
            use std::sync::atomic::AtomicU64;

            #[derive(Clone)]
            struct UsageStub {
                down: Arc<AtomicBool>,
                requests: Arc<AtomicU64>,
            }

            /// The DS-02 fixture's `/api/user` with `apiRequests` taken from the counter.
            async fn stub_user(
                State(stub): State<UsageStub>,
                AxumQuery(query): AxumQuery<Vec<(String, String)>>,
            ) -> Response {
                if stub.down.load(Ordering::SeqCst) {
                    return (StatusCode::SERVICE_UNAVAILABLE, "Service Unavailable")
                        .into_response();
                }
                let token = query
                    .iter()
                    .find(|(k, _)| k == "api_token")
                    .map(|(_, v)| v.as_str());
                if token != Some(TOKEN) {
                    return (StatusCode::UNAUTHORIZED, "Unauthenticated").into_response();
                }
                let body = fs::read_to_string(format!(
                    "{}/tests/fixtures/eodhd/user.json",
                    env!("CARGO_MANIFEST_DIR")
                ))
                .unwrap()
                .replace(
                    "\"apiRequests\": 1234",
                    &format!("\"apiRequests\": {}", stub.requests.load(Ordering::SeqCst)),
                );
                (StatusCode::OK, [("content-type", "application/json")], body).into_response()
            }

            async fn usage_stub() -> (String, Arc<AtomicBool>, Arc<AtomicU64>) {
                let down = Arc::new(AtomicBool::new(false));
                let requests = Arc::new(AtomicU64::new(1234));
                let router =
                    Router::new()
                        .route("/api/user", get(stub_user))
                        .with_state(UsageStub {
                            down: down.clone(),
                            requests: requests.clone(),
                        });
                (serve(router).await, down, requests)
            }

            fn usage_calls(state: &AppState, id: &str) -> (Option<u64>, Option<String>) {
                let row = load_source_row(state, id).unwrap().unwrap();
                (row.requests_today, row.usage_checked_at)
            }

            #[tokio::test]
            async fn the_card_shows_the_providers_usage_and_keeps_the_last_value_while_down() {
                let (eodhd, down, requests) = usage_stub().await;
                let root = scratch_root("credits");
                let library = root.join("library");
                fs::create_dir_all(&library).unwrap();
                let state = test_state(&root, &eodhd);
                let api = serve(api_router().with_state(state.clone())).await;
                let client = reqwest::Client::new();
                let mut responses: Vec<(String, String)> = Vec::new();

                // Registering reads the usage from the same call that verified the token.
                let response = client
                    .post(format!("{api}/api/sources"))
                    .json(&serde_json::json!({
                        "kind": "eodhd", "name": "EODHD", "root": library,
                        "catalog_dir": library.join("catalog"), "token": TOKEN
                    }))
                    .send()
                    .await
                    .unwrap();
                assert_eq!(response.status(), StatusCode::CREATED);
                let body = response.text().await.unwrap();
                let card: serde_json::Value = serde_json::from_str(&body).unwrap();
                responses.push(("POST created".into(), body));
                let id = card["id"].as_str().unwrap().to_owned();
                let usage = &card["usage"];
                assert_eq!(usage["requests_today"], 1234, "{card}");
                assert_eq!(usage["daily_limit"], 100_000);
                assert_eq!(usage["resets_at"], "2026-09-12T00:00:00+00:00");
                assert_eq!(usage["checked_at"], card["verified_at"]);
                assert_eq!(usage["reserve_calls"], 5_000);
                assert_eq!(usage["available_calls"], 100_000 - 1_234 - 5_000);
                let first_checked = usage["checked_at"].as_str().unwrap().to_owned();

                // Within the minute the listing shows what it has; `refresh=1` asks again.
                requests.store(2_000, Ordering::SeqCst);
                let body = client
                    .get(format!("{api}/api/sources"))
                    .send()
                    .await
                    .unwrap()
                    .text()
                    .await
                    .unwrap();
                let list: serde_json::Value = serde_json::from_str(&body).unwrap();
                responses.push(("GET sources cached".into(), body));
                assert_eq!(
                    list["sources"][0]["usage"]["requests_today"], 1234,
                    "{list}"
                );
                assert_eq!(
                    list["sources"][0]["usage"]["checked_at"],
                    first_checked.as_str()
                );
                let body = client
                    .get(format!("{api}/api/sources?refresh=1"))
                    .send()
                    .await
                    .unwrap()
                    .text()
                    .await
                    .unwrap();
                let list: serde_json::Value = serde_json::from_str(&body).unwrap();
                responses.push(("GET sources refreshed".into(), body));
                let card = &list["sources"][0];
                assert_eq!(card["usage"]["requests_today"], 2_000, "{card}");
                assert_eq!(card["usage"]["available_calls"], 100_000 - 2_000 - 5_000);
                assert_eq!(card["verify_state"], "connected");
                let refreshed_checked = card["usage"]["checked_at"].as_str().unwrap().to_owned();
                assert!(refreshed_checked >= first_checked);
                assert_eq!(
                    usage_calls(&state, &id),
                    (Some(2_000), Some(refreshed_checked.clone()))
                );

                // With the provider down, the card keeps the last figures with their time and
                // says the provider could not be reached.
                down.store(true, Ordering::SeqCst);
                requests.store(3_000, Ordering::SeqCst);
                let body = client
                    .get(format!("{api}/api/sources?refresh=1"))
                    .send()
                    .await
                    .unwrap()
                    .text()
                    .await
                    .unwrap();
                let list: serde_json::Value = serde_json::from_str(&body).unwrap();
                responses.push(("GET sources down".into(), body));
                let card = &list["sources"][0];
                assert_eq!(card["usage"]["requests_today"], 2_000, "{card}");
                assert_eq!(card["usage"]["checked_at"], refreshed_checked.as_str());
                assert_eq!(card["verify_state"], "unreachable");
                assert!(
                    card["verify_message"].as_str().unwrap().contains("503"),
                    "{card}"
                );
                assert!(
                    card["verified_at"].as_str().unwrap() >= refreshed_checked.as_str(),
                    "{card}"
                );
                // The token endpoint and the verify endpoint keep the figures too.
                let response = client
                    .post(format!("{api}/api/sources/{id}/verify"))
                    .send()
                    .await
                    .unwrap();
                assert_eq!(response.status(), StatusCode::OK);
                let body = response.text().await.unwrap();
                let card: serde_json::Value = serde_json::from_str(&body).unwrap();
                responses.push(("POST verify down".into(), body));
                assert_eq!(card["usage"]["requests_today"], 2_000, "{card}");
                assert_eq!(card["verify_state"], "unreachable");

                // Back up, a verify brings the figures current.
                down.store(false, Ordering::SeqCst);
                let body = client
                    .post(format!("{api}/api/sources/{id}/verify"))
                    .send()
                    .await
                    .unwrap()
                    .text()
                    .await
                    .unwrap();
                let card: serde_json::Value = serde_json::from_str(&body).unwrap();
                responses.push(("POST verify up".into(), body));
                assert_eq!(card["usage"]["requests_today"], 3_000, "{card}");
                assert_eq!(card["verify_state"], "connected");
                assert_eq!(card["usage"]["checked_at"], card["verified_at"]);

                // The reserve is set in place and recomputed in calls; out of range is refused.
                let response = client
                    .put(format!("{api}/api/sources/{id}"))
                    .json(&serde_json::json!({ "reserve_pct": 10 }))
                    .send()
                    .await
                    .unwrap();
                assert_eq!(response.status(), StatusCode::OK);
                let body = response.text().await.unwrap();
                let card: serde_json::Value = serde_json::from_str(&body).unwrap();
                responses.push(("PUT reserve".into(), body));
                assert_eq!(card["reserve_pct"], 10.0);
                assert_eq!(card["usage"]["reserve_calls"], 10_000);
                assert_eq!(card["usage"]["available_calls"], 100_000 - 3_000 - 10_000);
                assert_eq!(card["usage"]["requests_today"], 3_000);
                for bad in [-1.0, 100.5] {
                    let response = client
                        .put(format!("{api}/api/sources/{id}"))
                        .json(&serde_json::json!({ "reserve_pct": bad }))
                        .send()
                        .await
                        .unwrap();
                    assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{bad}");
                    responses.push((format!("PUT reserve {bad}"), response.text().await.unwrap()));
                }
                let response = client
                    .put(format!("{api}/api/sources/no-such-source"))
                    .json(&serde_json::json!({ "reserve_pct": 1 }))
                    .send()
                    .await
                    .unwrap();
                assert_eq!(response.status(), StatusCode::NOT_FOUND);
                let row = load_source_row(&state, &id).unwrap().unwrap();
                assert_eq!(row.reserve_pct, 10.0);
                let budget = budget_of(&row).unwrap();
                assert_eq!(
                    (budget.limit, budget.used, budget.reserve),
                    (100_000, 3_000, 10_000)
                );

                // A source whose token file is gone is reported, not failed on.
                fs::remove_file(root.join("data/ui/secrets").join(format!("{id}.token"))).unwrap();
                let body = client
                    .get(format!("{api}/api/sources?refresh=1"))
                    .send()
                    .await
                    .unwrap()
                    .text()
                    .await
                    .unwrap();
                let list: serde_json::Value = serde_json::from_str(&body).unwrap();
                responses.push(("GET sources no token".into(), body));
                let card = &list["sources"][0];
                assert_eq!(card["token_set"], false);
                assert_eq!(card["verify_state"], "unreachable");
                assert!(
                    card["verify_message"]
                        .as_str()
                        .unwrap()
                        .contains("no token is set"),
                    "{card}"
                );
                assert_eq!(card["usage"]["requests_today"], 3_000, "{card}");

                let secrets_path = root.join("data/ui/secrets").display().to_string();
                for (label, body) in &responses {
                    for leak in [TOKEN, secrets_path.as_str(), "data/ui/secrets"] {
                        assert!(!body.contains(leak), "{label} carries {leak:?}: {body}");
                    }
                }
                let _ = fs::remove_dir_all(&root);
            }

            #[test]
            fn a_source_is_checked_recently_within_the_cache_window() {
                let now = Utc.with_ymd_and_hms(2026, 9, 12, 12, 0, 0).unwrap();
                let at = |seconds: i64| (now - chrono::Duration::seconds(seconds)).to_rfc3339();
                assert!(checked_recently(Some(&at(0)), now));
                assert!(checked_recently(Some(&at(59)), now));
                assert!(!checked_recently(Some(&at(60)), now));
                assert!(!checked_recently(Some(&at(3_600)), now));
                assert!(
                    !checked_recently(Some(&at(-5)), now),
                    "a check in the future (a clock that went back) is not recent"
                );
                assert!(!checked_recently(Some("yesterday"), now));
                assert!(!checked_recently(None, now));
            }

            #[test]
            fn a_row_without_a_usage_report_has_no_budget_and_no_credits_line() {
                let mut row = SourceRow {
                    id: "s".into(),
                    name: "EODHD".into(),
                    kind: "eodhd".into(),
                    root: "/lib".into(),
                    catalog_dir: "/lib/catalog".into(),
                    reserve_pct: 5.0,
                    token_set_at: None,
                    verified_at: None,
                    verify_state: VERIFY_CONNECTED.into(),
                    verify_message: None,
                    created_at: "2026-09-12T00:00:00+00:00".into(),
                    requests_today: None,
                    daily_limit: None,
                    resets_at: None,
                    usage_checked_at: None,
                };
                assert!(budget_of(&row).is_none());
                assert!(usage_of(&row).is_none());
                row.requests_today = Some(10);
                row.daily_limit = Some(20);
                row.resets_at = Some("2026-09-13T00:00:00+00:00".into());
                row.usage_checked_at = Some("2026-09-12T01:02:03+00:00".into());
                let usage = usage_of(&row).unwrap();
                assert_eq!(
                    (usage.requests_today, usage.daily_limit, usage.reserve_calls),
                    (10, 20, 1)
                );
                assert_eq!(usage.available_calls, 9);
                assert_eq!(usage.resets_at, "2026-09-13T00:00:00+00:00");
                assert_eq!(usage.checked_at, "2026-09-12T01:02:03+00:00");
            }
        }

        /// DS-05 (decisions 0012, 0013, 0021): a dataset is registered against the cached
        /// listing, a scan in the background writes its figures and the Uncataloged folders,
        /// a folder that goes missing leaves the figures with the state Unavailable, and
        /// neither a dataset nor a source is removed while its files are on disk.
        mod datasets {
            use super::*;

            /// A service whose calendar symbol is SPY.US with its daily folder under
            /// `library`, so a scan's expected session comes from the files the test writes.
            fn test_state_over(root: &Path, eodhd_base_url: &str, library: &Path) -> AppState {
                let state = test_state(root, eodhd_base_url);
                let mut local = LocalConfig::bundled_example(Path::new(env!("CARGO_MANIFEST_DIR")));
                local.data.calendar_symbol = "SPY.US".to_owned();
                local.data.daily_dir = library.join("eod");
                AppState {
                    local: Arc::new(local),
                    ..state
                }
            }

            /// Polls the cards until no scan of `id` is running and returns its card.
            async fn settled_card(
                client: &reqwest::Client,
                api: &str,
                id: &str,
            ) -> serde_json::Value {
                for _ in 0..200 {
                    let (status, list, text) = call(client.get(format!("{api}/api/sources"))).await;
                    assert_eq!(status, StatusCode::OK, "{text}");
                    let card = list["sources"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .find(|card| card["id"] == id)
                        .cloned()
                        .expect("the source is listed");
                    if card["scanning"] == false {
                        return card;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(25)).await;
                }
                panic!("the scan of {id} did not finish");
            }

            fn dataset_count(state: &AppState) -> i64 {
                table_count(state, "datasets")
            }

            /// The DS-08 download endpoints beside the DS-02 stub: bulk bars for the two
            /// sessions after the seeded files (the recorded fixture with its date moved),
            /// an empty list for any other date, no splits, YHOO's history and none for
            /// TWTR, 404 for any other symbol; every call is counted, and while `hold` is
            /// set a bulk call waits, so a test can see the job running.
            #[derive(Clone)]
            struct JobStub {
                hold: Arc<AtomicBool>,
                calls: Arc<std::sync::atomic::AtomicU64>,
            }

            const JOB_SESSIONS: [&str; 2] = ["2026-09-10", "2026-09-11"];

            async fn stub_bulk(
                State(stub): State<JobStub>,
                AxumPath(exchange): AxumPath<String>,
                AxumQuery(query): AxumQuery<Vec<(String, String)>>,
            ) -> Response {
                stub.calls.fetch_add(1, Ordering::SeqCst);
                while stub.hold.load(Ordering::SeqCst) {
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
                let param = |name: &str| {
                    query
                        .iter()
                        .find(|(k, _)| k == name)
                        .map(|(_, v)| v.clone())
                        .unwrap_or_default()
                };
                let date = param("date");
                if exchange != "US"
                    || param("type") == "splits"
                    || !JOB_SESSIONS.contains(&date.as_str())
                {
                    return json_response(StatusCode::OK, "[]".to_owned());
                }
                json_response(
                    StatusCode::OK,
                    fixture("eod-bulk-last-day-US").replace("2026-09-11", &date),
                )
            }

            async fn stub_history(
                State(stub): State<JobStub>,
                AxumPath(symbol): AxumPath<String>,
            ) -> Response {
                stub.calls.fetch_add(1, Ordering::SeqCst);
                match symbol.as_str() {
                    "YHOO.US" => json_response(
                        StatusCode::OK,
                        r#"[{"date":"2020-01-02","open":30,"high":31,"low":29,"close":30.5,"adjusted_close":30.5,"volume":100},
                            {"date":"2020-01-03","open":30.5,"high":32,"low":30,"close":31,"adjusted_close":31,"volume":120}]"#
                            .to_owned(),
                    ),
                    "TWTR.US" => json_response(StatusCode::OK, "[]".to_owned()),
                    _ => json_response(
                        StatusCode::NOT_FOUND,
                        r#"{"message":"Symbol not found"}"#.to_owned(),
                    ),
                }
            }

            /// The DS-02 stub with the download endpoints merged in.
            async fn job_stub() -> (String, JobStub) {
                let jobs = JobStub {
                    hold: Arc::new(AtomicBool::new(false)),
                    calls: Arc::new(std::sync::atomic::AtomicU64::new(0)),
                };
                let router = Router::new()
                    .route("/api/user", get(stub_user))
                    .route("/api/exchanges-list/", get(stub_exchanges))
                    .route("/api/exchange-symbol-list/{exchange}", get(stub_symbols))
                    .with_state(Stub {
                        down: Arc::new(AtomicBool::new(false)),
                    })
                    .merge(
                        Router::new()
                            .route("/api/eod-bulk-last-day/{exchange}", get(stub_bulk))
                            .route("/api/eod/{symbol}", get(stub_history))
                            .with_state(jobs.clone()),
                    );
                (serve(router).await, jobs)
            }

            /// Polls a job until it is Complete or Failed.
            async fn finished_job(
                client: &reqwest::Client,
                api: &str,
                id: &str,
            ) -> serde_json::Value {
                for _ in 0..400 {
                    let (status, job, text) =
                        call(client.get(format!("{api}/api/datasets/jobs/{id}"))).await;
                    assert_eq!(status, StatusCode::OK, "{text}");
                    if job["state"] == "Complete" || job["state"] == "Failed" {
                        return job;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(25)).await;
                }
                panic!("job {id} did not finish");
            }

            /// Polls the cards until the source's rescan after a job has written its rows.
            async fn rescanned_card(
                client: &reqwest::Client,
                api: &str,
                id: &str,
            ) -> serde_json::Value {
                for _ in 0..400 {
                    let card = settled_card(client, api, id).await;
                    if card["scanned_at"].is_string() {
                        return card;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(25)).await;
                }
                panic!("the source {id} was not rescanned");
            }

            fn seeded_daily(dates: &[&str]) -> String {
                let mut text = "Date,Open,High,Low,Close,Adjusted_close,Volume\n".to_owned();
                for (i, date) in dates.iter().enumerate() {
                    text.push_str(&format!(
                        "{date},{0},{1},{2},{0},{0},1000\n",
                        100 + i,
                        101 + i,
                        99 + i
                    ));
                }
                text
            }

            #[test]
            fn the_calendar_code_is_the_symbol_on_its_own_exchange_only() {
                assert_eq!(calendar_code_for("SPY.US", "US").as_deref(), Some("SPY"));
                assert_eq!(calendar_code_for("SPY.US", "LSE"), None);
                assert_eq!(
                    calendar_code_for("BRK-B.US", "US").as_deref(),
                    Some("BRK-B")
                );
                assert_eq!(calendar_code_for("SPY", "US"), None);
                assert_eq!(calendar_code_for(".US", "US"), None);
                let today = today_in_new_york();
                let utc = Utc::now().date_naive();
                assert!(today == utc || today == utc.pred_opt().unwrap());
            }

            /// DS-08 (decisions 0020, 0022): `POST /api/datasets/{id}/update` queues the EOD
            /// job over the stub; while it runs the dataset says Updating and a second POST
            /// on the source is 409 naming the job; when it ends the files carry the two
            /// sessions, the delisted symbol with a history is backfilled and the one
            /// without is skipped with its reason, the catalog files are regenerated from
            /// the cached listing, the log downloads, the source's usage is refreshed, and
            /// the source is rescanned. A removed folder or an unmounted root is 409 before
            /// any call.
            #[tokio::test]
            async fn a_datasets_update_runs_the_eod_job_in_the_background_and_refuses_a_second() {
                let (eodhd, jobs) = job_stub().await;
                let root = scratch_root("jobs");
                let library = root.join("library");
                let eod = library.join("eod");
                let catalog = library.join("catalog");
                fs::create_dir_all(&eod).unwrap();
                fs::create_dir_all(&catalog).unwrap();
                fs::write(catalog.join("stocks.txt"), "STALE.US\n").unwrap();
                let seed_dates = ["2026-09-08", "2026-09-09"];
                for code in ["AAPL", "SPY", "BRK-B"] {
                    fs::write(
                        eod.join(format!("{code}.US.csv")),
                        seeded_daily(&seed_dates),
                    )
                    .unwrap();
                }
                let state = test_state_over(&root, &eodhd, &library);
                let api = serve(api_router().with_state(state.clone())).await;
                let client = reqwest::Client::new();
                let secrets_path = root.join("data/ui/secrets").display().to_string();
                let leaks = [TOKEN, secrets_path.as_str(), "data/ui/secrets"];
                let mut responses: Vec<(String, String)> = Vec::new();

                let (status, card, text) = call(client.post(format!("{api}/api/sources")).json(
                    &serde_json::json!({
                        "kind": "eodhd", "name": "EODHD", "root": library,
                        "catalog_dir": catalog, "token": TOKEN
                    }),
                ))
                .await;
                assert_eq!(status, StatusCode::CREATED, "{text}");
                responses.push(("POST source".into(), text));
                let id = card["id"].as_str().unwrap().to_owned();
                let (status, _, text) = call(
                    client
                        .post(format!("{api}/api/sources/{id}/availability/refresh"))
                        .json(&serde_json::json!({ "exchange": "US", "delisted": true })),
                )
                .await;
                assert_eq!(status, StatusCode::OK, "{text}");
                let (status, dataset, text) = call(
                    client
                        .post(format!("{api}/api/sources/{id}/datasets"))
                        .json(&serde_json::json!({
                            "exchange": "US", "types": ["Common Stock", "ETF"],
                            "resolution": "daily", "from_date": "2020-01-01",
                            "min_bulk_rows": 3
                        })),
                )
                .await;
                assert_eq!(status, StatusCode::CREATED, "{text}");
                responses.push(("POST dataset".into(), text));
                let dataset_id = dataset["id"].as_str().unwrap().to_owned();
                assert_eq!(dataset["min_bulk_rows"], 3);
                assert!(dataset["last_job"].is_null(), "{dataset}");
                let (status, intraday, text) = call(
                    client
                        .post(format!("{api}/api/sources/{id}/datasets"))
                        .json(&serde_json::json!({
                            "exchange": "US", "types": ["ETF"], "resolution": "5m",
                            "from_date": "2024-01-01"
                        })),
                )
                .await;
                assert_eq!(status, StatusCode::CREATED, "{text}");
                assert_eq!(intraday["min_bulk_rows"], DEFAULT_MIN_BULK_ROWS);
                let intraday_id = intraday["id"].as_str().unwrap().to_owned();

                // The job is queued at once and the stub holds its first bulk call.
                jobs.hold.store(true, Ordering::SeqCst);
                let update = format!("{api}/api/datasets/{dataset_id}/update");
                let (status, job, text) = call(client.post(&update)).await;
                assert_eq!(status, StatusCode::ACCEPTED, "{text}");
                responses.push(("POST update".into(), text));
                let job_id = job["id"].as_str().unwrap().to_owned();
                assert!(job_id.starts_with("job-"), "{job}");
                assert_eq!(job["dataset_id"], dataset_id.as_str());
                assert_eq!(job["source_id"], id.as_str());
                assert_eq!(job["kind"], "eod");
                assert_eq!(job["state"], "Queued");
                assert_eq!(job["percent"], 0);
                assert_eq!(job["calls"], 0);
                assert_eq!(job["skipped"], serde_json::json!([]));
                assert!(job["finished_at"].is_null(), "{job}");
                assert_eq!(
                    job["log_path"],
                    format!("data/ui/logs/{job_id}.log").as_str()
                );

                // A second job on the source is refused naming the running one, whichever
                // dataset it is for; the dataset says Updating meanwhile.
                let (status, _, text) = call(client.post(&update)).await;
                assert_eq!(status, StatusCode::CONFLICT, "{text}");
                assert!(text.contains(&job_id), "{text}");
                responses.push(("POST update while running".into(), text));
                let (status, _, text) =
                    call(client.post(format!("{api}/api/datasets/{intraday_id}/update"))).await;
                assert!(
                    status == StatusCode::CONFLICT || status == StatusCode::BAD_REQUEST,
                    "{text}"
                );
                let (_, list, text) = call(client.get(format!("{api}/api/sources"))).await;
                responses.push(("GET sources updating".into(), text));
                let running = &list["sources"][0]["datasets"][0];
                assert_eq!(running["state"], "Updating", "{running}");
                assert_eq!(running["last_job"]["id"], job_id.as_str());
                let (status, _, text) =
                    call(client.delete(format!("{api}/api/datasets/{dataset_id}"))).await;
                assert_eq!(status, StatusCode::CONFLICT, "{text}");
                assert!(text.contains(&job_id), "{text}");
                let (status, _, text) =
                    call(client.delete(format!("{api}/api/sources/{id}"))).await;
                assert_eq!(status, StatusCode::CONFLICT, "{text}");
                assert!(text.contains(&job_id), "{text}");

                jobs.hold.store(false, Ordering::SeqCst);
                let job = finished_job(&client, &api, &job_id).await;
                responses.push(("GET job".into(), job.to_string()));
                assert_eq!(job["state"], "Complete", "{job}");
                assert_eq!(job["percent"], 100);
                assert_eq!(job["added"], 1, "{job}");
                assert_eq!(job["updated"], 3, "{job}");
                assert!(job["calls"].as_u64().unwrap() >= 5, "{job}");
                assert_eq!(job["skipped"].as_array().unwrap().len(), 1, "{job}");
                assert_eq!(job["skipped"][0]["symbol"], "TWTR.US");
                assert!(
                    job["skipped"][0]["reason"]
                        .as_str()
                        .unwrap()
                        .contains("no history"),
                    "{job}"
                );
                assert!(job["estimate"]["mandatory"].as_u64().unwrap() >= 4, "{job}");
                assert_eq!(job["estimate"]["optional"], 2);
                assert!(job["error"].is_null(), "{job}");
                assert!(job["started_at"].is_string() && job["finished_at"].is_string());
                let finished_at = job["finished_at"].as_str().unwrap().to_owned();
                let calls_after_job = jobs.calls.load(Ordering::SeqCst);
                assert!(calls_after_job >= 5, "{calls_after_job}");

                // The files: two sessions on the three, YHOO backfilled, TWTR absent.
                for code in ["AAPL", "SPY", "BRK-B"] {
                    let text = fs::read_to_string(eod.join(format!("{code}.US.csv"))).unwrap();
                    let lines: Vec<&str> = text.lines().collect();
                    assert_eq!(lines.len(), 5, "{code}:\n{text}");
                    assert!(lines[3].starts_with("2026-09-10,"), "{text}");
                    assert!(lines[4].starts_with("2026-09-11,"), "{text}");
                }
                let yhoo = fs::read_to_string(eod.join("YHOO.US.csv")).unwrap();
                assert_eq!(
                    yhoo,
                    "Date,Open,High,Low,Close,Adjusted_close,Volume\n\
                     2020-01-02,30,31,29,30.5,30.5,100\n2020-01-03,30.5,32,30,31,31,120\n"
                );
                assert!(!eod.join("TWTR.US.csv").exists());
                for entry in fs::read_dir(&eod).unwrap().flatten() {
                    let name = entry.file_name().to_string_lossy().into_owned();
                    assert!(!name.ends_with(".part"), "part file left: {name}");
                }
                // The catalog files come from the cached listing, in today's columns.
                assert_eq!(
                    fs::read_to_string(catalog.join("catalog.csv")).unwrap(),
                    "Code,Name,Country,Exchange,Currency,Type\n\
                     AAPL,Apple Inc,USA,NASDAQ,USD,Common Stock\n\
                     BRK-B,Berkshire Hathaway Inc,USA,NYSE,USD,Common Stock\n\
                     SPY,SPDR S&P 500 ETF Trust,USA,NYSE ARCA,USD,ETF\n"
                );
                assert_eq!(
                    fs::read_to_string(catalog.join("stocks.txt")).unwrap(),
                    "AAPL.US\nBRK-B.US\n"
                );
                assert_eq!(
                    fs::read_to_string(catalog.join("etfs.txt")).unwrap(),
                    "SPY.US\n"
                );

                // The log downloads from under data/ui/, never from the dataset folder.
                let response = client
                    .get(format!("{api}/api/datasets/jobs/{job_id}/log"))
                    .send()
                    .await
                    .unwrap();
                assert_eq!(response.status(), StatusCode::OK);
                let disposition = response
                    .headers()
                    .get("content-disposition")
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .to_owned();
                assert!(disposition.contains("attachment"), "{disposition}");
                let log = response.text().await.unwrap();
                assert!(log.contains("complete:"), "{log}");
                assert!(log.contains("backfilled"), "{log}");
                responses.push(("GET log".into(), log));
                assert!(root.join(format!("data/ui/logs/{job_id}.log")).is_file());
                assert!(!eod.join(format!("{job_id}.log")).exists());
                let (status, _, text) =
                    call(client.get(format!("{api}/api/datasets/jobs/nope"))).await;
                assert_eq!(status, StatusCode::NOT_FOUND, "{text}");

                // After the job the usage was asked again and the source rescanned.
                let row = load_source_row(&state, &id).unwrap().unwrap();
                assert!(
                    row.usage_checked_at.as_deref().unwrap() >= finished_at.as_str(),
                    "{:?} < {finished_at}",
                    row.usage_checked_at
                );
                let card = rescanned_card(&client, &api, &id).await;
                responses.push(("GET sources rescanned".into(), card.to_string()));
                let daily = &card["datasets"][0];
                assert_eq!(daily["last_job"]["id"], job_id.as_str());
                assert_eq!(daily["last_job"]["state"], "Complete");
                assert_eq!(daily["state"], "Partial", "{daily}");
                assert_eq!(daily["scan"]["on_disk"], 4, "{daily}");
                assert_eq!(daily["scan"]["latest_date"], "2026-09-11");
                assert_eq!(daily["scan"]["current_count"], 3);
                assert_eq!(table_count(&state, "dataset_jobs"), 1);

                // A removed folder is refused before any call, and not created; so is an
                // unmounted root.
                let calls_before = jobs.calls.load(Ordering::SeqCst);
                fs::remove_dir_all(&eod).unwrap();
                let (status, _, text) = call(client.post(&update)).await;
                assert_eq!(status, StatusCode::CONFLICT, "{text}");
                assert!(
                    text.contains("missing") && text.contains("never creates"),
                    "{text}"
                );
                responses.push(("POST update folder missing".into(), text));
                assert!(!eod.exists(), "the folder was created");
                fs::rename(&library, root.join("unmounted")).unwrap();
                let (status, _, text) = call(client.post(&update)).await;
                assert_eq!(status, StatusCode::CONFLICT, "{text}");
                assert!(text.contains("not mounted"), "{text}");
                responses.push(("POST update root unmounted".into(), text));
                assert_eq!(
                    jobs.calls.load(Ordering::SeqCst),
                    calls_before,
                    "a call was made"
                );
                assert_eq!(table_count(&state, "dataset_jobs"), 1);
                let (status, _, text) =
                    call(client.post(format!("{api}/api/datasets/nope/update"))).await;
                assert_eq!(status, StatusCode::NOT_FOUND, "{text}");

                for (label, body) in &responses {
                    for leak in &leaks {
                        assert!(!body.contains(leak), "{label} carries {leak:?}: {body}");
                    }
                }
                let _ = fs::remove_dir_all(&root);
            }

            #[tokio::test]
            async fn datasets_are_scanned_in_the_background_and_the_figures_outlive_a_missing_folder()
             {
                let (eodhd, down) = stub().await;
                let root = scratch_root("datasets");
                let library = root.join("library");
                let eod = library.join("eod");
                fs::create_dir_all(&eod).unwrap();
                fs::create_dir_all(library.join("catalog")).unwrap();
                fs::write(library.join("catalog/catalog.csv"), "Code,Name\nSPY,SPDR\n").unwrap();
                let state = test_state_over(&root, &eodhd, &library);
                let api = serve(api_router().with_state(state.clone())).await;
                let client = reqwest::Client::new();
                let secrets_path = root.join("data/ui/secrets").display().to_string();
                let leaks = [TOKEN, secrets_path.as_str(), "data/ui/secrets"];
                let mut responses: Vec<(String, String)> = Vec::new();

                let (status, card, text) = call(client.post(format!("{api}/api/sources")).json(
                    &serde_json::json!({
                        "kind": "eodhd", "name": "EODHD", "root": library,
                        "catalog_dir": library.join("catalog"), "token": TOKEN
                    }),
                ))
                .await;
                assert_eq!(status, StatusCode::CREATED, "{text}");
                responses.push(("POST source".into(), text));
                let id = card["id"].as_str().unwrap().to_owned();
                assert_eq!(card["datasets"], serde_json::json!([]));
                assert_eq!(card["uncataloged"], serde_json::json!([]));
                assert!(
                    card["scanned_at"].is_null() && card["scanning"] == false,
                    "{card}"
                );

                // A second source over the same root is refused before the provider is asked.
                down.store(true, Ordering::SeqCst);
                let (status, _, text) = call(client.post(format!("{api}/api/sources")).json(
                    &serde_json::json!({
                        "kind": "eodhd", "name": "Again", "root": format!("{}/", library.display()),
                        "catalog_dir": library.join("catalog"), "token": TOKEN
                    }),
                ))
                .await;
                assert_eq!(status, StatusCode::CONFLICT, "{text}");
                assert!(text.contains("EODHD"), "{text}");
                responses.push(("POST same root".into(), text));
                down.store(false, Ordering::SeqCst);
                assert_eq!(source_count(&state), 1);

                // The refresh with the delisted flag caches the two delisted rows apart: the
                // listing of five symbols the dataset counts against.
                let availability = format!("{api}/api/sources/{id}/availability");
                let (status, table, text) = call(
                    client
                        .post(format!("{availability}/refresh"))
                        .json(&serde_json::json!({ "exchange": "US", "delisted": true })),
                )
                .await;
                assert_eq!(status, StatusCode::OK, "{text}");
                responses.push(("POST refresh delisted".into(), text));
                let us = table["exchanges"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|e| e["code"] == "US")
                    .unwrap()
                    .clone();
                assert_eq!(us["listed"], 3, "{us}");
                assert_eq!(us["delisted"], 2, "{us}");
                assert!(us["delisted_fetched_at"].is_string(), "{us}");
                assert_eq!(table_count(&state, "provider_listings"), 5);

                // Registration is validated against the cache.
                let datasets = format!("{api}/api/sources/{id}/datasets");
                let body = |exchange: &str, types: &[&str], resolution: &str| {
                    serde_json::json!({
                        "exchange": exchange, "types": types, "resolution": resolution,
                        "from_date": "2020-01-01"
                    })
                };
                for (label, request, expected, note) in [
                    (
                        "unknown exchange",
                        body("MARS", &["ETF"], "daily"),
                        StatusCode::BAD_REQUEST,
                        "MARS",
                    ),
                    (
                        "resolution not offered",
                        body("US", &["ETF"], "1s"),
                        StatusCode::BAD_REQUEST,
                        "1s",
                    ),
                    (
                        "listing not cached",
                        body("LSE", &["ETF"], "daily"),
                        StatusCode::CONFLICT,
                        "LSE",
                    ),
                    (
                        "unknown type",
                        body("US", &["Bond"], "daily"),
                        StatusCode::BAD_REQUEST,
                        "Bond",
                    ),
                    (
                        "bad date",
                        serde_json::json!({"exchange": "US", "types": ["ETF"], "resolution": "daily", "from_date": "yesterday"}),
                        StatusCode::BAD_REQUEST,
                        "from_date",
                    ),
                    (
                        "no types",
                        serde_json::json!({"exchange": "US", "types": [], "resolution": "daily", "from_date": "2020-01-01"}),
                        StatusCode::BAD_REQUEST,
                        "type",
                    ),
                ] {
                    let (status, _, text) = call(client.post(&datasets).json(&request)).await;
                    assert_eq!(status, expected, "{label}: {text}");
                    assert!(text.contains(note), "{label}: {text}");
                    responses.push((format!("POST dataset {label}"), text));
                }
                assert_eq!(dataset_count(&state), 0);
                let (status, _, text) = call(
                    client
                        .post(format!("{api}/api/sources/nope/datasets"))
                        .json(&body("US", &["ETF"], "daily")),
                )
                .await;
                assert_eq!(status, StatusCode::NOT_FOUND, "{text}");

                // A US EOD dataset of both types: the folder defaults to <root>/eod and delisted
                // symbols are included for daily bars.
                let (status, dataset, text) = call(client.post(&datasets).json(&body(
                    "US",
                    &["ETF", "Common Stock"],
                    "daily",
                )))
                .await;
                assert_eq!(status, StatusCode::CREATED, "{text}");
                responses.push(("POST dataset".into(), text));
                let dataset_id = dataset["id"].as_str().unwrap().to_owned();
                assert_eq!(dataset["source_id"], id.as_str());
                assert_eq!(dataset["exchange"], "US");
                assert_eq!(dataset["types"], serde_json::json!(["Common Stock", "ETF"]));
                assert_eq!(dataset["resolution"], "daily");
                assert_eq!(dataset["from_date"], "2020-01-01");
                assert_eq!(dataset["folder"], eod.display().to_string().as_str());
                assert_eq!(dataset["include_delisted"], true);
                assert_eq!(dataset["state"], "Unknown");
                assert!(dataset["scan"].is_null(), "{dataset}");

                // A dataset already covering a type on that exchange and resolution is refused;
                // the same type at another resolution is not.
                let (status, _, text) =
                    call(client.post(&datasets).json(&body("US", &["ETF"], "daily"))).await;
                assert_eq!(status, StatusCode::CONFLICT, "{text}");
                assert!(text.contains(&dataset_id), "{text}");
                responses.push(("POST dataset overlap".into(), text));
                let (status, intraday, text) =
                    call(client.post(&datasets).json(&serde_json::json!({
                        "exchange": "US", "types": ["ETF"], "resolution": "5m",
                        "from_date": "2024-01-01", "folder": "bars/5m"
                    })))
                    .await;
                assert_eq!(status, StatusCode::CREATED, "{text}");
                responses.push(("POST dataset 5m".into(), text));
                let intraday_id = intraday["id"].as_str().unwrap().to_owned();
                assert_eq!(
                    intraday["folder"],
                    library.join("bars/5m").display().to_string().as_str()
                );
                assert_eq!(intraday["include_delisted"], false);
                assert_eq!(dataset_count(&state), 2);

                // The card lists both, unscanned.
                let (_, list, text) = call(client.get(format!("{api}/api/sources"))).await;
                responses.push(("GET sources unscanned".into(), text));
                let card = &list["sources"][0];
                assert_eq!(card["datasets"].as_array().unwrap().len(), 2);
                assert_eq!(card["datasets"][0]["id"], dataset_id.as_str());
                assert_eq!(card["datasets"][0]["state"], "Unknown");
                assert!(card["scanned_at"].is_null(), "{card}");

                // Three listed files, one part file, a stray folder, a loose file at the root.
                let files = [
                    ("AAPL.US.csv", "Date,Close\n2026-09-10,1\n2026-09-11,2\n"),
                    (
                        "SPY.US.csv",
                        "Date,Close\n2026-09-09,1\n2026-09-10,2\n2026-09-11,3\n",
                    ),
                    ("BRK-B.US.csv", "Date,Close\n2026-09-09,1\n2026-09-10,2\n\n"),
                ];
                let mut bytes = 0u64;
                for (name, text) in files {
                    fs::write(eod.join(name), text).unwrap();
                    bytes += text.len() as u64;
                }
                fs::write(eod.join("YHOO.US.csv.part"), "Date,Close\n2026-09-11,1\n").unwrap();
                fs::create_dir_all(library.join("stray/sub")).unwrap();
                fs::write(library.join("stray/a.csv"), "a").unwrap();
                fs::write(library.join("stray/sub/b.csv"), "bb").unwrap();
                fs::write(library.join("README.txt"), "notes").unwrap();

                // The scan is accepted at once and runs in the background.
                let (status, accepted, text) =
                    call(client.post(format!("{api}/api/sources/{id}/scan"))).await;
                assert_eq!(status, StatusCode::ACCEPTED, "{text}");
                responses.push(("POST scan".into(), text));
                assert_eq!(accepted["source_id"], id.as_str());
                assert_eq!(accepted["scanning"], true);
                let (status, _, text) =
                    call(client.post(format!("{api}/api/sources/nope/scan"))).await;
                assert_eq!(status, StatusCode::NOT_FOUND, "{text}");
                let card = settled_card(&client, &api, &id).await;
                responses.push(("GET sources scanned".into(), card.to_string()));
                let scanned_at = card["scanned_at"].as_str().unwrap().to_owned();
                let daily = &card["datasets"][0];
                assert_eq!(daily["state"], "Partial", "{daily}");
                let scan = &daily["scan"];
                assert_eq!(scan["listed"], 5, "{scan}");
                assert_eq!(scan["on_disk"], 3, "{scan}");
                assert_eq!(scan["latest_date"], "2026-09-11", "{scan}");
                assert_eq!(scan["current_count"], 2, "{scan}");
                assert_eq!(scan["bytes"], bytes, "{scan}");
                assert_eq!(scan["scanned_at"], scanned_at.as_str());
                assert!(scan["error"].is_null(), "{scan}");
                assert_eq!(
                    card["uncataloged"],
                    serde_json::json!([
                        { "folder": ".", "files": 1, "bytes": 5 },
                        { "folder": "stray", "files": 2, "bytes": 3 }
                    ]),
                    "{card}"
                );
                // The 5m dataset's folder does not exist: Unavailable with nothing counted.
                let intraday = &card["datasets"][1];
                assert_eq!(intraday["state"], "Unavailable", "{intraday}");
                assert_eq!(intraday["scan"]["on_disk"], 0);
                assert_eq!(intraday["scan"]["scanned_at"], scanned_at.as_str());

                // Neither the dataset nor the source goes while the files are there.
                let (status, _, text) =
                    call(client.delete(format!("{api}/api/datasets/{dataset_id}"))).await;
                assert_eq!(status, StatusCode::CONFLICT, "{text}");
                assert!(text.contains("never deletes"), "{text}");
                responses.push(("DELETE dataset refused".into(), text));
                let (status, _, text) =
                    call(client.delete(format!("{api}/api/sources/{id}"))).await;
                assert_eq!(status, StatusCode::CONFLICT, "{text}");
                responses.push(("DELETE source refused".into(), text));
                assert_eq!(dataset_count(&state), 2);
                assert_eq!(source_count(&state), 1);

                // With the folder gone, a rescan says Unavailable and keeps the figures.
                fs::remove_dir_all(&eod).unwrap();
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                let (status, _, text) =
                    call(client.post(format!("{api}/api/sources/{id}/scan"))).await;
                assert_eq!(status, StatusCode::ACCEPTED, "{text}");
                let card = settled_card(&client, &api, &id).await;
                responses.push(("GET sources rescanned".into(), card.to_string()));
                let rescanned_at = card["scanned_at"].as_str().unwrap().to_owned();
                assert!(rescanned_at > scanned_at, "{rescanned_at} <= {scanned_at}");
                let daily = &card["datasets"][0];
                assert_eq!(daily["state"], "Unavailable", "{daily}");
                let scan = &daily["scan"];
                assert_eq!(scan["scanned_at"], rescanned_at.as_str());
                assert_eq!(scan["listed"], 5, "{scan}");
                assert_eq!(scan["on_disk"], 3, "{scan}");
                assert_eq!(scan["latest_date"], "2026-09-11", "{scan}");
                assert_eq!(scan["current_count"], 2, "{scan}");
                assert_eq!(scan["bytes"], bytes, "{scan}");
                assert!(scan["error"].is_null(), "{scan}");
                assert_eq!(card["uncataloged"].as_array().unwrap().len(), 2, "{card}");

                // Without files the dataset goes, and the card comes back without it.
                let (status, card, text) =
                    call(client.delete(format!("{api}/api/datasets/{dataset_id}"))).await;
                assert_eq!(status, StatusCode::OK, "{text}");
                responses.push(("DELETE dataset".into(), text));
                assert_eq!(card["id"], id.as_str());
                assert_eq!(card["datasets"].as_array().unwrap().len(), 1);
                assert_eq!(card["datasets"][0]["id"], intraday_id.as_str());
                assert_eq!(dataset_count(&state), 1);
                assert_eq!(table_count(&state, "dataset_scans"), 1);
                let (status, _, text) =
                    call(client.delete(format!("{api}/api/datasets/{dataset_id}"))).await;
                assert_eq!(status, StatusCode::NOT_FOUND, "{text}");

                // The source's guard is its dataset folders, not the root: the stray files
                // do not hold it, and its dataset and scan go with it.
                let (status, _, text) =
                    call(client.delete(format!("{api}/api/sources/{id}"))).await;
                assert_eq!(status, StatusCode::NO_CONTENT, "{text}");
                assert_eq!(source_count(&state), 0);
                assert_eq!(dataset_count(&state), 0);
                assert_eq!(table_count(&state, "dataset_scans"), 0);
                assert!(
                    library.join("stray/a.csv").is_file(),
                    "a data file was deleted"
                );

                for (label, body) in &responses {
                    for leak in &leaks {
                        assert!(!body.contains(leak), "{label} carries {leak:?}: {body}");
                    }
                }
                let _ = fs::remove_dir_all(&root);
            }

            #[test]
            fn the_state_follows_the_listing_the_files_and_the_expected_session() {
                assert_eq!(scan_state(0, 0, 0, true), STATE_UNKNOWN);
                assert_eq!(scan_state(5, 5, 5, false), STATE_UNKNOWN);
                assert_eq!(scan_state(5, 0, 0, true), STATE_PARTIAL);
                assert_eq!(scan_state(5, 3, 2, true), STATE_PARTIAL);
                assert_eq!(scan_state(5, 3, 0, true), STATE_STALE);
                assert_eq!(scan_state(100, 94, 90, true), STATE_PARTIAL);
                assert_eq!(scan_state(100, 95, 1, true), STATE_CURRENT);
                assert_eq!(scan_state(100, 100, 100, true), STATE_CURRENT);
                assert_eq!(scan_state(100, 100, 0, true), STATE_STALE);
            }

            #[test]
            fn the_expected_session_is_a_date_for_daily_bars_and_the_close_for_intraday() {
                let session = NaiveDate::from_ymd_opt(2026, 9, 11).unwrap();
                let daily = ExpectedSession::for_resolution(session, "daily");
                assert_eq!(daily, ExpectedSession::Date(session));
                assert!(daily.reached_by("2026-09-11"));
                assert!(daily.reached_by("2026-09-14"));
                assert!(!daily.reached_by("2026-09-10"));
                assert!(!daily.reached_by("Date"));

                // 2026-09-11 16:00 in New York is 20:00 UTC (EDT), epoch 1789156800.
                let close = 1_789_156_800;
                let five = ExpectedSession::for_resolution(session, "5m");
                assert_eq!(
                    five,
                    ExpectedSession::Close {
                        session,
                        epoch: close - 300
                    }
                );
                assert!(five.reached_by(&(close - 300).to_string()));
                assert!(!five.reached_by(&(close - 600).to_string()));
                assert!(
                    five.reached_by("2026-09-11"),
                    "a dated row falls back to the session"
                );
                assert!(!five.reached_by("2026-09-10"));
                let hourly = ExpectedSession::for_resolution(session, "1h");
                assert_eq!(
                    hourly,
                    ExpectedSession::Close {
                        session,
                        epoch: close - 3600
                    }
                );
                assert_eq!(bar_seconds("1m"), Some(60));
                assert_eq!(bar_seconds("daily"), None);
                assert_eq!(bar_seconds(""), None);
                assert_eq!(default_dataset_folder("daily"), "eod");
                assert_eq!(default_dataset_folder("5m"), "5m");
                assert_eq!(symbol_file_name("BRK-B", "US"), "BRK-B.US.csv");
            }

            #[test]
            fn the_last_row_field_is_tail_read_past_blank_lines() {
                let root = scratch_root("tail");
                fs::create_dir_all(&root).unwrap();
                let daily = root.join("d.csv");
                fs::write(&daily, "Date,Close\n2026-09-10,1\n2026-09-11,2\n\n\n").unwrap();
                assert_eq!(last_csv_row_field(&daily).as_deref(), Some("2026-09-11"));
                assert_eq!(last_csv_row_date(&daily).as_deref(), Some("2026-09-11"));
                let intraday = root.join("i.csv");
                fs::write(&intraday, "Timestamp,Close\n1789156500,1\n").unwrap();
                assert_eq!(last_csv_row_field(&intraday).as_deref(), Some("1789156500"));
                assert_eq!(last_csv_row_date(&intraday).as_deref(), Some("2026-09-11"));
                let empty = root.join("e.csv");
                fs::write(&empty, "").unwrap();
                assert_eq!(last_csv_row_field(&empty), None);
                assert_eq!(last_csv_row_field(&root.join("missing.csv")), None);
                let _ = fs::remove_dir_all(&root);
            }

            #[test]
            fn uncataloged_folders_skip_claimed_ones_and_walk_into_those_above_them() {
                let root = scratch_root("uncataloged");
                for dir in ["eod", "us/5m", "us/1m", "stray/sub", "empty", "catalog"] {
                    fs::create_dir_all(root.join(dir)).unwrap();
                }
                fs::write(root.join("eod/SPY.US.csv"), "1").unwrap();
                fs::write(root.join("us/5m/SPY.US.csv"), "22").unwrap();
                fs::write(root.join("us/1m/SPY.US.csv"), "333").unwrap();
                fs::write(root.join("us/notes.txt"), "4444").unwrap();
                fs::write(root.join("stray/a.csv"), "1").unwrap();
                fs::write(root.join("stray/sub/b.csv"), "22").unwrap();
                fs::write(root.join("catalog/catalog.csv"), "c").unwrap();
                fs::write(root.join("loose.txt"), "55555").unwrap();
                let claimed = [root.join("eod"), root.join("us/5m"), root.join("catalog")];
                let found = uncataloged_folders(&root, &claimed);
                let rows: Vec<(&str, u64, u64)> = found
                    .iter()
                    .map(|f| (f.folder.as_str(), f.files, f.bytes))
                    .collect();
                assert_eq!(
                    rows,
                    [(".", 1, 5), ("stray", 2, 3), ("us", 1, 4), ("us/1m", 1, 3)]
                );
                assert!(uncataloged_folders(&root.join("missing"), &claimed).is_empty());
                assert_eq!(count_files(&root.join("stray")), (2, 3));
                let _ = fs::remove_dir_all(&root);
            }

            #[test]
            fn a_scan_counts_only_the_listed_files_and_reads_their_last_dates() {
                let root = scratch_root("scan");
                fs::create_dir_all(&root).unwrap();
                fs::write(root.join("AAPL.US.csv"), "Date,Close\n2026-09-11,1\n").unwrap();
                fs::write(root.join("SPY.US.csv"), "Date,Close\n2026-09-10,1\n").unwrap();
                fs::write(root.join("SPY.US.csv.part"), "Date,Close\n2026-09-11,1\n").unwrap();
                fs::write(root.join("ZZZ.US.csv"), "Date,Close\n2026-09-12,1\n").unwrap();
                let symbols: Vec<String> = ["AAPL", "SPY", "MSFT"].map(String::from).to_vec();
                let session = NaiveDate::from_ymd_opt(2026, 9, 11).unwrap();
                let figures =
                    scan_dataset(&root, "US", &symbols, Some(ExpectedSession::Date(session)))
                        .unwrap();
                assert_eq!(
                    figures,
                    ScanFigures {
                        listed: 3,
                        on_disk: 2,
                        latest_date: Some("2026-09-11".into()),
                        current_count: 1,
                        bytes: 2 * "Date,Close\n2026-09-11,1\n".len() as u64,
                        state: STATE_PARTIAL,
                    }
                );
                let unjudged = scan_dataset(&root, "US", &symbols, None).unwrap();
                assert_eq!((unjudged.current_count, unjudged.state), (0, STATE_UNKNOWN));
                assert!(scan_dataset(&root.join("missing"), "US", &symbols, None).is_err());
                assert!(dataset_holds_files(&root, "US", &symbols));
                assert!(!dataset_holds_files(&root, "US", &["MSFT".to_owned()]));
                assert!(
                    dataset_holds_files(&root, "US", &[]),
                    "no listing: any file holds"
                );
                let _ = fs::remove_dir_all(&root);
            }

            #[test]
            fn a_scan_that_cannot_judge_keeps_the_previous_figures() {
                let connection = Connection::open_in_memory().unwrap();
                migrate(&connection).unwrap();
                connection
                    .execute_batch(
                        "INSERT INTO data_sources (id, name, kind, root, catalog_dir, created_at)
                         VALUES ('s1', 'EODHD', 'eodhd', '/tmp/x', '/tmp/x/catalog', 't0');
                         INSERT INTO datasets (id, source_id, exchange, types_json, resolution,
                                               from_date, folder, include_delisted, created_at)
                         VALUES ('d1', 's1', 'US', '[\"ETF\"]', 'daily', '2020-01-01',
                                 '/tmp/x/eod', 1, 't0'),
                                ('d2', 's1', 'US', '[\"ETF\"]', '5m', '2020-01-01',
                                 '/tmp/x/5m', 0, 't1');",
                    )
                    .unwrap();
                let figures = ScanFigures {
                    listed: 5,
                    on_disk: 3,
                    latest_date: Some("2026-09-11".into()),
                    current_count: 2,
                    bytes: 90,
                    state: STATE_PARTIAL,
                };
                store_figures(&connection, "d1", "t2", &figures, "[]").unwrap();
                keep_figures(&connection, "d2", "t2", STATE_UNAVAILABLE, "[]", None).unwrap();
                let rows = load_datasets(&connection, "s1").unwrap();
                assert_eq!(rows.len(), 2);
                let d1 = rows[0].scan.as_ref().unwrap();
                assert_eq!(
                    (rows[0].state.as_str(), d1.listed, d1.on_disk),
                    ("Partial", 5, 3)
                );
                assert_eq!(d1.latest_date.as_deref(), Some("2026-09-11"));
                assert_eq!(
                    (d1.current_count, d1.bytes, d1.scanned_at.as_str()),
                    (2, 90, "t2")
                );
                let d2 = rows[1].scan.as_ref().unwrap();
                assert_eq!(rows[1].state, "Unavailable");
                assert_eq!(
                    (d2.listed, d2.on_disk, d2.bytes, d2.scanned_at.as_str()),
                    (0, 0, 0, "t2")
                );
                assert!(d2.latest_date.is_none() && d2.error.is_none());

                // A later scan that fails keeps d1's figures, moves the time, and says why; the
                // Uncataloged folders travel with the newest row.
                let stray = r#"[{"folder":"stray","files":2,"bytes":3}]"#;
                keep_figures(
                    &connection,
                    "d1",
                    "t3",
                    STATE_FAILED,
                    stray,
                    Some("read the dataset folder: denied"),
                )
                .unwrap();
                let rows = load_datasets(&connection, "s1").unwrap();
                let d1 = rows[0].scan.as_ref().unwrap();
                assert_eq!(rows[0].state, "Failed");
                assert_eq!(
                    (d1.listed, d1.on_disk, d1.current_count, d1.bytes),
                    (5, 3, 2, 90)
                );
                assert_eq!(d1.latest_date.as_deref(), Some("2026-09-11"));
                assert_eq!(d1.scanned_at, "t3");
                assert_eq!(d1.error.as_deref(), Some("read the dataset folder: denied"));
                let (scanned_at, uncataloged) = last_scan_of(&connection, "s1").unwrap();
                assert_eq!(scanned_at.as_deref(), Some("t3"));
                assert_eq!(
                    uncataloged,
                    [UncatalogedFolder {
                        folder: "stray".into(),
                        files: 2,
                        bytes: 3
                    }]
                );
                assert_eq!(
                    last_scan_of(&connection, "nobody").unwrap(),
                    (None, Vec::new())
                );

                // A scan that judges again replaces the row whole, error cleared.
                store_figures(
                    &connection,
                    "d1",
                    "t4",
                    &ScanFigures {
                        state: STATE_CURRENT,
                        ..figures
                    },
                    "[]",
                )
                .unwrap();
                let d1 = load_dataset(&connection, "d1").unwrap().unwrap();
                assert_eq!(d1.state, "Current");
                assert!(d1.scan.unwrap().error.is_none());
                assert!(load_dataset(&connection, "d9").unwrap().is_none());

                // The listing a dataset counts against: its types, delisted when included.
                connection
                    .execute_batch(
                        "INSERT INTO provider_listings
                         (source_id, exchange, code, name, type, currency, delisted, fetched_at)
                         VALUES ('s1', 'US', 'SPY', '', 'ETF', 'USD', 0, 't'),
                                ('s1', 'US', 'AAPL', '', 'Common Stock', 'USD', 0, 't'),
                                ('s1', 'US', 'OLD', '', 'ETF', 'USD', 1, 't'),
                                ('s1', 'LSE', 'ISF', '', 'ETF', 'GBP', 0, 't');",
                    )
                    .unwrap();
                let rows = load_datasets(&connection, "s1").unwrap();
                assert_eq!(
                    listed_symbols(&connection, &rows[0]).unwrap(),
                    ["OLD", "SPY"]
                );
                assert_eq!(listed_symbols(&connection, &rows[1]).unwrap(), ["SPY"]);
            }
        }
    }
}
