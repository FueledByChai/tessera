"use client";

import {
  FormEvent,
  Fragment,
  KeyboardEvent,
  useCallback,
  useEffect,
  useMemo,
  useState,
} from "react";

const ORIGIN = "http://127.0.0.1:8787";
const API = `${ORIGIN}/api`;
const monthNames = [
  "Jan",
  "Feb",
  "Mar",
  "Apr",
  "May",
  "Jun",
  "Jul",
  "Aug",
  "Sep",
  "Oct",
  "Nov",
  "Dec",
];
const comparisonPalette = ["#1f9cff", "#ffb000", "#00d26a", "#ff4057"];

type View =
  | "dashboard"
  | "strategies"
  | "runs"
  | "compare"
  | "portfolios"
  | "strategy"
  | "run"
  | "data"
  | "costs"
  | "code";
type Strategy = {
  id: string;
  name: string;
  version: string;
  status: string;
  description: string;
  asset_scope: string;
  config_path: string;
  runnable: boolean;
  base_strategy_id?: string;
  source_sha256?: string;
  custom: boolean;
};
type Run = {
  id: string;
  strategy_id?: string;
  name: string;
  research_label: string;
  status: string;
  legacy: boolean;
  report_path?: string;
  artifact_dir: string;
  config_path?: string;
  start_date?: string;
  end_date?: string;
  created_at: string;
  starred: boolean;
  metrics_cached: boolean;
  metrics?: RunMetrics | null;
};
type RunMetrics = {
  cagr_percent?: number | null;
  total_return_percent?: number | null;
  sharpe?: number | null;
  sortino?: number | null;
  calmar?: number | null;
  max_drawdown_percent?: number | null;
  annual_volatility_percent?: number | null;
  win_rate_percent?: number | null;
  trades?: number | null;
  start?: string | null;
  end?: string | null;
};
type JobProgress = {
  stage: string;
  done: number;
  total: number;
  percent: number;
  label: string;
  elapsed_seconds: number;
};
type Job = {
  id: string;
  strategy_id: string;
  status: string;
  start_date: string;
  end_date: string;
  created_at: string;
  log_path: string;
  error?: string;
  progress?: JobProgress | null;
};

function formatElapsed(seconds: number) {
  const m = Math.floor(seconds / 60);
  const s = seconds % 60;
  return m > 0 ? `${m}m ${String(s).padStart(2, "0")}s` : `${s}s`;
}

/** Progress of a running engine job: stage, bar, counts, and a rough time remaining. */
function JobProgressBar({ job }: { job: Job }) {
  const progress = job.progress;
  if (job.status === "queued") {
    return <div className="job-progress"><span className="job-progress-stage">QUEUED</span><span>waiting for a worker</span></div>;
  }
  if (!progress) {
    return <div className="job-progress"><span className="job-progress-stage">STARTING</span><span>loading data…</span></div>;
  }
  const pct = Math.max(0, Math.min(100, progress.percent));
  const rate = progress.elapsed_seconds > 0 && progress.done > 0 ? progress.done / progress.elapsed_seconds : 0;
  const remaining = rate > 0 ? Math.round((progress.total - progress.done) / rate) : null;
  const unit = progress.stage === "replay" ? "sessions" : progress.stage === "load" ? "symbols" : progress.stage;
  return (
    <div className="job-progress">
      <div className="job-progress-row">
        <span className="job-progress-stage">{progress.stage.toUpperCase()}</span>
        <span className="job-progress-pct">{pct.toFixed(0)}%</span>
      </div>
      <div className="job-progress-track"><i style={{ width: `${pct}%` }} /></div>
      <div className="job-progress-row">
        <span>{progress.done.toLocaleString()} / {progress.total.toLocaleString()} {unit}{progress.stage === "replay" && progress.label && progress.label !== "done" ? ` · ${progress.label}` : ""}</span>
        <span>{formatElapsed(progress.elapsed_seconds)}{remaining != null && pct < 100 ? ` · ~${formatElapsed(remaining)} left` : ""}</span>
      </div>
    </div>
  );
}
type Dashboard = {
  strategies: Strategy[];
  recent_runs: Run[];
  jobs: Job[];
  production_strategies: number;
  historical_reports: number;
  active_jobs: number;
  worker_capacity: number;
};
type Overrides = {
  [key: string]: number | boolean | string | string[] | undefined;
  asset?: string;
  symbols?: string[];
  gap_stdev_lookback?: number;
  realized_vol_lookback?: number;
  regime_median_lookback?: number;
  require_vol_regime?: boolean;
  minimum_absolute_gap_z?: number;
  stop_loss_percent?: number;
  stop_realized_vol_multiple?: number;
  adaptive_stop_floor_percent?: number;
  adaptive_stop_cap_percent?: number;
  minimum_gap_retracement_fraction?: number;
  slippage_ticks?: number;
  commission_per_share?: number;
  selection_method?: string;
  target_annualized_volatility?: number;
  maximum_leverage?: number;
  volatility_target_lookback?: number;
  top_n?: number;
  minimum_volume_thrust?: number;
  minimum_daily_return?: number;
  minimum_close_location?: number;
  minimum_average_dollar_volume?: number;
  require_regime?: boolean;
  regime_method?: string;
  minimum_short_momentum_return?: number;
  minimum_long_momentum_return?: number;
  maximum_distance_from_high?: number;
  minimum_annualized_realized_volatility?: number;
  target_annualized_portfolio_volatility?: number;
  maximum_exposure_multiplier?: number;
  minimum_adx?: number;
  minimum_atr_percent?: number;
  limit_atr_multiple?: number;
  entry_window_minutes?: number;
  gap_veto_buffer_percent?: number;
  holding_period_minutes?: number;
  position_percent?: number;
  min_price?: number;
  max_gross_exposure?: number;
  max_positions_per_day?: number;
  all_in_round_trip_bps?: number;
  position_slots?: number;
  minimum_return_zscore?: number;
  minimum_decline_from_high?: number;
  require_gap_up?: boolean;
  entry_z?: number;
  history_sessions?: number;
  minimum_history_sessions?: number;
  trend_lookback_sessions?: number;
  fast_length?: number;
  slow_length?: number;
  risk_percent?: number;
  maximum_gross_exposure?: number;
  maximum_shares?: number;
  entry_slippage_ticks?: number;
  exit_slippage_ticks?: number;
  commission_per_share_per_fill?: number;
};
type Preset = {
  id: string;
  name: string;
  parameters: Overrides;
  costs_enabled: boolean;
  created_at: string;
};
type InstrumentRequirement = {
  parameter: "asset" | "symbols";
  mode: "single" | "multiple";
  resolutions: string[];
  suffixes: string[];
  asset_classes: string[];
  maximum: number;
  note: string;
};
type InstrumentHit = {
  symbol: string;
  code: string;
  suffix: string;
  name: string;
  exchange: string;
  asset_class: string;
  currency: string;
  status: string;
  daily: boolean;
  five_minute: boolean;
  one_minute: boolean;
  coverage: Record<string, { first: string; last: string }>;
  missing_resolutions: string[];
};
type SdkParam = {
  name: string;
  label: string;
  help: string;
  tier: string;
  unit: string;
  kind: "int" | "decimal" | "bool" | "choice";
  default: number | boolean | string;
  min?: number | null;
  max?: number | null;
  step?: number | null;
  choices?: string[];
};
type SdkManifest = {
  id: string;
  name: string;
  version: string;
  description: string;
  rules: string[];
  asset_scope: string;
  warmup_bars: number;
  allows_short: boolean;
  daily_context?: boolean;
  screen_universe?: boolean;
  default_max_entries_per_day?: number | null;
  default_tie_break?: string | null;
  default_seed?: number;
  default_symbols?: string[];
  default_resolution?: string | null;
  params: SdkParam[];
};
type StrategyDetail = {
  strategy: Strategy;
  rules: string[];
  default_parameters: Record<string, unknown>;
  presets: Preset[];
  runs: Run[];
  instruments?: InstrumentRequirement | null;
  sdk?: SdkManifest | null;
};
type ReportView = {
  strategy_name: string;
  resolution: string;
  currency: string;
  start: string;
  end: string;
  symbols: string[];
  parameters: Record<string, string>;
  correlation_matrices: {
    frequency: string;
    observations: number;
    labels: string[];
    values: number[][];
  }[];
  metrics: {
    initial_capital: number;
    ending_equity: number;
    total_return_percent: number;
    cagr_percent: number;
    sharpe?: number;
    sortino?: number;
    calmar?: number;
    max_drawdown_percent: number;
    annual_volatility_percent: number;
    win_rate_percent: number;
    profit_factor?: number;
    average_trade_pnl: number;
    best_trade_percent: number;
    worst_trade_percent: number;
    average_hold_minutes: number;
    max_daily_fills: number;
    average_leverage?: number;
    maximum_leverage?: number;
  };
  coverage: {
    covered: number;
    missing_file: number;
    missing_session: number;
    total: number;
    percent: number;
  };
  coverage_rows: { trade_date: string; symbol: string; status: string }[];
  coverage_missing_total?: number;
  coverage_by_symbol?: { key: string; covered: number; total: number }[];
  coverage_by_year?: { key: string; covered: number; total: number }[];
  watchlist: {
    watch_date: string;
    symbol: string;
    momentum_rank: number;
    prior_close: number;
    prior_average_dollar_volume: number;
    prior_short_momentum_return_percent: number;
    prior_long_momentum_return_percent: number;
    prior_annualized_realized_volatility_percent: number;
    prior_distance_from_high_percent: number;
  }[];
  daily: {
    date: string;
    equity: number;
    daily_return_percent: number;
    drawdown_percent: number;
    fills: number;
  }[];
  trades: {
    symbol: string;
    direction?: string;
    trade_date: string;
    entry_time: string;
    exit_time: string;
    pnl: number;
    return_percent: number;
    leverage?: number;
    entry_price?: number | null;
    exit_price?: number | null;
    quantity?: number | null;
  }[];
  trade_breakdown: {
    scope: string;
    trades: number;
    win_rate_percent: number;
    total_pnl: number;
    average_pnl: number;
    average_return_percent: number;
    profit_factor?: number;
  }[];
  yearly_returns: {
    year: number;
    months: (number | null)[];
    annual_return_percent: number;
    annual_drawdown_percent: number;
  }[];
};
type RunDetail = {
  run: Run;
  report?: ReportView;
  report_url?: string;
  config_text?: string;
  manifest?: Record<string, unknown>;
};
type DataUpdate = {
  id: string;
  status: string;
  created_at: string;
  started_at?: string;
  finished_at?: string;
  log_path: string;
  error?: string;
};
type DataStatus = {
  latest_market_date: string;
  latest_spy_date: string;
  symbols_on_latest_date: number;
  universe_symbols: number;
  updated_at_utc: string;
  update_job?: DataUpdate;
};
type SweepAxis = { parameter: string; values: number[] };
type SweepRecord = {
  id: string;
  strategy_id: string;
  name: string;
  research_label: string;
  start_date: string;
  end_date: string;
  axes: SweepAxis[];
  costs_enabled: boolean;
  created_at: string;
  status: string;
  configuration_count: number;
  complete_count: number;
  failed_count: number;
};
type SweepMember = {
  configuration_index: number;
  run_id: string;
  job_id: string;
  status: string;
  parameters: Record<string, number>;
  metrics?: {
    sharpe?: number;
    cagr_percent: number;
    max_drawdown_percent: number;
    annual_volatility_percent: number;
    trade_count: number;
  };
};
type SweepDetail = { sweep: SweepRecord; members: SweepMember[] };
type PortfolioRecord = {
  id: string;
  run_id: string;
  name: string;
  capital_mode: string;
  initial_capital: number;
  created_at: string;
  component_count: number;
};
type CostProfile = {
  id: string;
  name: string;
  asset_class: string;
  model: string;
  entry_bps: number;
  exit_bps: number;
  tick_size: number;
  entry_slippage_ticks: number;
  exit_slippage_ticks: number;
  entry_commission_per_unit: number;
  exit_commission_per_unit: number;
  minimum_commission: number;
  created_at: string;
  builtin: boolean;
};
type AutomationSchedule = {
  id: string;
  name: string;
  kind: string;
  enabled: boolean;
  local_time: string;
  weekdays: string;
  last_run_date?: string;
  last_status?: string;
  created_at: string;
};
type StrategySourceFile = {
  path: string;
  content: string;
  editable: boolean;
};
type StrategySource = {
  strategy: Strategy;
  files: StrategySourceFile[];
  source_sha256: string;
  immutable: boolean;
};
type StrategyDraft = {
  id: string;
  base_strategy_id: string;
  name: string;
  version: string;
  description: string;
  status: string;
  source_sha256: string;
  created_at: string;
  updated_at: string;
  last_validation_id?: string;
  release_strategy_id?: string;
};
type StrategyValidation = {
  id: string;
  draft_id: string;
  action: string;
  status: string;
  source_sha256: string;
  created_at: string;
  started_at?: string;
  finished_at?: string;
  log: string;
  error?: string;
};
type StrategyDraftDetail = {
  draft: StrategyDraft;
  files: StrategySourceFile[];
  validation?: StrategyValidation;
};

const emptyDashboard: Dashboard = {
  strategies: [],
  recent_runs: [],
  jobs: [],
  production_strategies: 0,
  historical_reports: 0,
  active_jobs: 0,
  worker_capacity: 2,
};
const navItems: [string, string, View | "new"][] = [
  ["Dashboard", "⌂", "dashboard"],
  ["Strategies", "◆", "strategies"],
  ["New Backtest", "＋", "new"],
  ["Runs", "▤", "runs"],
  ["Compare", "⇄", "compare"],
  ["Portfolios", "◫", "portfolios"],
  ["Data", "▦", "data"],
  ["Costs", "¢", "costs"],
  ["Code", "</>", "code"],
];

function Metric({
  label,
  value,
  note,
}: {
  label: string;
  value: string;
  note: string;
}) {
  const sign = value.trim().startsWith("+")
    ? "positive-text"
    : /^[-\u2212]/.test(value.trim())
      ? "negative-text"
      : "";
  return (
    <article className="metric-card">
      <p>{label}</p>
      <strong className={sign}>{value}</strong>
      <span>{note}</span>
    </article>
  );
}
function number(value: number | undefined, digits = 2) {
  return value == null || !Number.isFinite(value) ? "—" : value.toFixed(digits);
}
function percent(value: number | undefined, digits = 2) {
  return value == null || !Number.isFinite(value)
    ? "—"
    : `${value >= 0 ? "+" : ""}${value.toFixed(digits)}%`;
}
function money(value: number | undefined) {
  return value == null
    ? "—"
    : new Intl.NumberFormat("en-US", {
        style: "currency",
        currency: "USD",
        maximumFractionDigits: 0,
      }).format(value);
}
/** Prices with enough decimals to show sub-dollar fills honestly. */
function tradePrice(value: number) {
  const decimals = Math.abs(value) >= 1 ? 2 : 4;
  return value.toLocaleString(undefined, { minimumFractionDigits: decimals, maximumFractionDigits: decimals });
}

function classFor(value: number | undefined) {
  return value == null ? "" : value >= 0 ? "positive-text" : "negative-text";
}

/** Ticking wall clock, rendered only after mount so server and client markup agree. */
function Clock({ withZone = true }: { withZone?: boolean }) {
  const [now, setNow] = useState<Date | null>(null);
  useEffect(() => {
    const tick = () => setNow(new Date());
    const first = window.setTimeout(tick, 0);
    const timer = window.setInterval(tick, 1000);
    return () => {
      window.clearTimeout(first);
      window.clearInterval(timer);
    };
  }, []);
  if (!now) return <time className="clock">--/--/-- --:--:--</time>;
  const day = now.toLocaleDateString(undefined, { weekday: "short" }).toUpperCase();
  const date = now.toLocaleDateString(undefined, {
    month: "2-digit",
    day: "2-digit",
    year: "2-digit",
  });
  const time = now.toLocaleTimeString(undefined, { hour12: false });
  const zone = withZone
    ? (now
        .toLocaleTimeString(undefined, { timeZoneName: "short" })
        .split(" ")
        .pop() ?? "")
    : "";
  return (
    <time className="clock" dateTime={now.toISOString()}>
      {day} {date} {time}
      {zone ? ` ${zone}` : ""}
    </time>
  );
}

function formatNumber(value?: number | null, digits = 1) {
  return value == null || Number.isNaN(value) ? "—" : value.toFixed(digits);
}

function EquityChart({ rows }: { rows: ReportView["daily"] }) {
  const [hover, setHover] = useState<number | null>(null);
  const geometry = useMemo(() => {
    if (!rows.length)
      return { points: "", coordinates: [] as [number, number][], min: 0, span: 1 };
    const values = rows.map((row) => Math.log(Math.max(row.equity, 0.01)));
    const min = Math.min(...values);
    const max = Math.max(...values);
    const span = Math.max(max - min, 0.0001);
    const coordinates = values.map(
      (value, index) =>
        [
          32 + (index / Math.max(rows.length - 1, 1)) * 736,
          206 - ((value - min) / span) * 172,
        ] as [number, number],
    );
    return {
      points: coordinates.map(([x, y]) => `${x},${y}`).join(" "),
      coordinates,
      min,
      span,
    };
  }, [rows]);
  if (!rows.length)
    return <div className="empty-state">No daily equity observations.</div>;
  const gridValue = (y: number) =>
    Math.exp(geometry.min + ((206 - y) / 172) * geometry.span);
  const active = hover == null ? rows.length - 1 : hover;
  const [activeX, activeY] = geometry.coordinates[active];
  return (
    <div className="equity-chart">
      <div className="chart-readout">
        <strong>{rows[active].date}</strong>
        <span>{money(rows[active].equity)}</span>
        <span className={classFor(rows[active].drawdown_percent)}>
          DD {percent(rows[active].drawdown_percent)}
        </span>
      </div>
      <svg
        viewBox="0 0 800 240"
        role="img"
        aria-label="Logarithmic equity curve"
        onPointerMove={(event) => {
          const rect = event.currentTarget.getBoundingClientRect();
          const ratio = Math.min(
            1,
            Math.max(0, (event.clientX - rect.left) / rect.width),
          );
          setHover(Math.round(ratio * (rows.length - 1)));
        }}
        onPointerLeave={() => setHover(null)}
      >
        {[34, 77, 120, 163, 206].map((y) => (
          <g key={y}>
            <line x1="32" y1={y} x2="768" y2={y} className="chart-grid" />
            <text x="766" y={y - 3} textAnchor="end" className="chart-axis">
              {money(gridValue(y))}
            </text>
          </g>
        ))}
        <polygon
          points={`32,206 ${geometry.points} 768,206`}
          className="chart-area"
        />
        <polyline points={geometry.points} className="chart-line" />
        <line
          x1={activeX}
          y1="28"
          x2={activeX}
          y2="210"
          className="chart-crosshair"
        />
        <line
          x1="32"
          y1={activeY}
          x2="768"
          y2={activeY}
          className="chart-crosshair"
        />
        <circle cx={activeX} cy={activeY} r="4" className="chart-point" />
        <text x="32" y="228" className="chart-label">
          {rows[0].date}
        </text>
        <text x="768" y="228" textAnchor="end" className="chart-label">
          {rows.at(-1)?.date}
        </text>
      </svg>
    </div>
  );
}

function RunReport({ detail }: { detail: RunDetail }) {
  const report = detail.report;
  if (!report)
    return (
      <section className="panel legacy-report-card">
        <h2>Original HTML report</h2>
        <p>
          This legacy run does not yet expose structured result data. Its
          preserved report is still available.
        </p>
        {detail.report_url && (
          <a
            className="primary-action link-button"
            href={`${ORIGIN}${detail.report_url}`}
            target="_blank"
            rel="noreferrer"
          >
            Open report ↗
          </a>
        )}
      </section>
    );
  const m = report.metrics;
  return (
    <>
      {report.coverage.percent < 99.5 && (
        <div className="coverage-warning">
          <strong>Partial signal-session coverage</strong>
          <span>
            {report.coverage.covered} of {report.coverage.total} signal sessions
            were covered ({number(report.coverage.percent, 2)}%). Treat
            performance as coverage-dependent.
          </span>
        </div>
      )}
      <section className="run-kpis">
        <Metric
          label="CAGR"
          value={percent(m.cagr_percent)}
          note={`Total ${percent(m.total_return_percent)}`}
        />
        <Metric
          label="Sharpe"
          value={number(m.sharpe)}
          note={`Sortino ${number(m.sortino)}`}
        />
        <Metric
          label="Max drawdown"
          value={percent(m.max_drawdown_percent)}
          note={`Calmar ${number(m.calmar)}`}
        />
        <Metric
          label="Annual volatility"
          value={percent(m.annual_volatility_percent)}
          note={`Target from run config`}
        />
        <Metric
          label="Trades"
          value={String(report.trades.length)}
          note={`${number(m.win_rate_percent, 1)}% winners`}
        />
      </section>
      <section className="panel report-panel">
        <div className="panel-head">
          <div>
            <p className="eyebrow">Log scale</p>
            <h2>Portfolio equity</h2>
          </div>
          <span className="coverage-pill">
            {number(report.coverage.percent, 1)}% coverage
          </span>
        </div>
        <EquityChart rows={report.daily} />
      </section>
      <div className="report-grid">
        <section className="panel">
          <div className="panel-head">
            <div>
              <p className="eyebrow">Direction audit</p>
              <h2>All / long / short</h2>
            </div>
          </div>
          <div className="table-wrap">
            <table>
              <thead>
                <tr>
                  <th>Scope</th>
                  <th>Trades</th>
                  <th>Win</th>
                  <th>Net P&amp;L</th>
                  <th>Avg return</th>
                  <th>PF</th>
                </tr>
              </thead>
              <tbody>
                {report.trade_breakdown.map((row) => (
                  <tr key={row.scope}>
                    <td>{row.scope}</td>
                    <td>{row.trades}</td>
                    <td>{number(row.win_rate_percent, 1)}%</td>
                    <td className={classFor(row.total_pnl)}>
                      {money(row.total_pnl)}
                    </td>
                    <td className={classFor(row.average_return_percent)}>
                      {percent(row.average_return_percent, 3)}
                    </td>
                    <td>{number(row.profit_factor)}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </section>
        <section className="panel">
          <div className="panel-head">
            <div>
              <p className="eyebrow">Execution summary</p>
              <h2>Risk and fills</h2>
            </div>
          </div>
          <dl className="detail-list">
            <div>
              <dt>Ending equity</dt>
              <dd>{money(m.ending_equity)}</dd>
            </div>
            <div>
              <dt>Profit factor</dt>
              <dd>{number(m.profit_factor)}</dd>
            </div>
            <div>
              <dt>Average leverage</dt>
              <dd>{number(m.average_leverage)}×</dd>
            </div>
            <div>
              <dt>Maximum leverage</dt>
              <dd>{number(m.maximum_leverage)}×</dd>
            </div>
            <div>
              <dt>Best trade</dt>
              <dd className={classFor(m.best_trade_percent)}>
                {percent(m.best_trade_percent, 3)}
              </dd>
            </div>
            <div>
              <dt>Worst trade</dt>
              <dd className={classFor(m.worst_trade_percent)}>
                {percent(m.worst_trade_percent, 3)}
              </dd>
            </div>
          </dl>
        </section>
      </div>
      <section className="panel monthly-panel">
        <div className="panel-head">
          <div>
            <p className="eyebrow">Compounded returns</p>
            <h2>Monthly and annual performance</h2>
          </div>
        </div>
        <div className="table-wrap">
          <table className="heatmap">
            <thead>
              <tr>
                <th>Year</th>
                {monthNames.map((month) => (
                  <th key={month}>{month}</th>
                ))}
                <th>Annual</th>
                <th>Max DD</th>
              </tr>
            </thead>
            <tbody>
              {report.yearly_returns.map((row) => (
                <tr key={row.year}>
                  <td>{row.year}</td>
                  {row.months.map((value, index) => (
                    <td key={index} className={classFor(value ?? undefined)}>
                      {value == null ? "—" : percent(value, 1)}
                    </td>
                  ))}
                  <td className={classFor(row.annual_return_percent)}>
                    {percent(row.annual_return_percent, 1)}
                  </td>
                  <td className={classFor(row.annual_drawdown_percent)}>
                    {percent(row.annual_drawdown_percent, 1)}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      </section>
      {report.correlation_matrices.length > 0 && (
        <section className="correlation-grid-ui">
          {report.correlation_matrices.map((matrix) => (
            <div className="panel correlation-panel-ui" key={matrix.frequency}>
              <div className="terminal-panel-title">
                <span>COR</span> {matrix.frequency.toUpperCase()} RETURN CORRELATION
                <small>{matrix.observations} OBS</small>
              </div>
              <div className="table-wrap">
                <table className="correlation-table-ui">
                  <thead>
                    <tr>
                      <th />
                      {matrix.labels.map((label) => <th key={label}>{label}</th>)}
                    </tr>
                  </thead>
                  <tbody>
                    {matrix.labels.map((label, rowIndex) => (
                      <tr key={label}>
                        <th>{label}</th>
                        {matrix.values[rowIndex].map((value, columnIndex) => (
                          <td
                            key={matrix.labels[columnIndex]}
                            className={
                              !Number.isFinite(value)
                                ? "pending-cell"
                                : value >= 0
                                  ? "heat-positive"
                                  : "heat-negative"
                            }
                          >
                            {number(value, 2)}
                          </td>
                        ))}
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            </div>
          ))}
        </section>
      )}
      <section className="panel trade-panel">
        <div className="panel-head">
          <div>
            <p className="eyebrow">Audit trail</p>
            <h2>Trades</h2>
          </div>
          <span className="market-date">{report.trades.length} TOTAL</span>
        </div>
        <div className="table-wrap">
          <table>
            <thead>
              <tr>
                <th>Entry</th>
                <th>Exit</th>
                <th>Symbol</th>
                <th>Side</th>
                <th>Shares</th>
                <th>Entry px</th>
                <th>Exit px</th>
                <th>Return</th>
                <th>P&amp;L</th>
                <th>Leverage</th>
              </tr>
            </thead>
            <tbody>
              {report.trades.map((trade, index) => (
                  <tr key={`${trade.trade_date}-${trade.symbol}-${index}`}>
                    <td>{trade.trade_date} {trade.entry_time.slice(11, 16)}</td>
                    <td>{trade.exit_time.slice(0, 10)} {trade.exit_time.slice(11, 16)}</td>
                    <td>{trade.symbol}</td>
                    <td>
                      <span className={`direction ${trade.direction}`}>
                        {trade.direction ?? "—"}
                      </span>
                    </td>
                    <td>{trade.quantity != null ? trade.quantity.toLocaleString() : "—"}</td>
                    <td>{trade.entry_price != null ? tradePrice(trade.entry_price) : "—"}</td>
                    <td>{trade.exit_price != null ? tradePrice(trade.exit_price) : "—"}</td>
                    <td className={classFor(trade.return_percent)}>
                      {percent(trade.return_percent, 3)}
                    </td>
                    <td className={classFor(trade.pnl)}>{money(trade.pnl)}</td>
                    <td>{number(trade.leverage)}×</td>
                  </tr>
                ))}
            </tbody>
          </table>
        </div>
      </section>
      {report.watchlist.length > 0 && (
        <section className="panel trade-panel">
          <div className="panel-head">
            <div>
              <p className="eyebrow">Production handoff</p>
              <h2>Recent generated watchlist</h2>
            </div>
            <span className="market-date">LATEST 1,000 ROWS</span>
          </div>
          <div className="table-wrap">
            <table>
              <thead>
                <tr>
                  <th>Date</th>
                  <th>Rank</th>
                  <th>Symbol</th>
                  <th>Prior close</th>
                  <th>ADV</th>
                  <th>20d momentum</th>
                  <th>60d momentum</th>
                  <th>Realized vol</th>
                  <th>From high</th>
                </tr>
              </thead>
              <tbody>
                {report.watchlist
                  .slice()
                  .reverse()
                  .map((row, index) => (
                    <tr key={`${row.watch_date}-${row.symbol}-${index}`}>
                      <td>{row.watch_date}</td>
                      <td>{row.momentum_rank}</td>
                      <td>{row.symbol}</td>
                      <td>{money(row.prior_close)}</td>
                      <td>{money(row.prior_average_dollar_volume)}</td>
                      <td>
                        {percent(row.prior_short_momentum_return_percent, 1)}
                      </td>
                      <td>
                        {percent(row.prior_long_momentum_return_percent, 1)}
                      </td>
                      <td>
                        {percent(
                          row.prior_annualized_realized_volatility_percent,
                          1,
                        )}
                      </td>
                      <td>
                        {percent(row.prior_distance_from_high_percent, 1)}
                      </td>
                    </tr>
                  ))}
              </tbody>
            </table>
          </div>
        </section>
      )}
    </>
  );
}

function DataCoverage({
  runs,
  detail,
  onSelect,
}: {
  runs: Run[];
  detail: RunDetail | null;
  onSelect: (id: string) => void;
}) {
  const report = detail?.report;
  // Aggregates come from the API; the raw rows are only the missing symbol-dates, capped.
  const missing = useMemo(
    () => (report?.coverage_rows ?? []).filter((row) => row.status !== "covered"),
    [report?.coverage_rows],
  );
  const missingTotal = report?.coverage_missing_total ?? missing.length;
  const SYMBOL_TABLE_CAP = 2000;
  const bySymbol = useMemo(
    () => (report?.coverage_by_symbol ?? []).map((b) => [b.key, b] as const),
    [report?.coverage_by_symbol],
  );
  const byYear = useMemo(
    () => (report?.coverage_by_year ?? []).map((b) => [b.key, b] as const),
    [report?.coverage_by_year],
  );
  const structured = runs.filter(
    (run) => !run.legacy && run.status === "Complete",
  );
  return (
    <>
      <div className="section-intro">
        <p className="eyebrow">Evidence quality</p>
        <h2>Data coverage</h2>
        <p>
          This audits the exact symbol-dates selected by each signal. It is
          signal-session coverage, not a claim that every market session is
          present.
        </p>
      </div>
      <section className="panel coverage-selector">
        <label>
          Completed structured run
          <select
            value={detail?.run.id ?? ""}
            onChange={(event) => onSelect(event.target.value)}
          >
            {structured.map((run) => (
              <option key={run.id} value={run.id}>
                {run.name}
              </option>
            ))}
          </select>
        </label>
      </section>
      {!report ? (
        <section className="panel empty-state">
          Select a completed UI run with structured coverage data.
        </section>
      ) : (
        <>
          <section className="run-kpis">
            <Metric
              label="Signal-session coverage"
              value={`${number(report.coverage.percent, 2)}%`}
              note={`${report.coverage.covered} of ${report.coverage.total} selected rows`}
            />
            <Metric
              label="Missing sessions"
              value={String(report.coverage.missing_session)}
              note="Symbol-date lacks required bars"
            />
            <Metric
              label="Missing files"
              value={String(report.coverage.missing_file)}
              note="Required source file absent"
            />
            <Metric
              label="Symbols"
              value={String(bySymbol.length)}
              note={report.symbols.join(", ") || "Cross-sectional universe"}
            />
            <Metric
              label="Missing rows"
              value={String(missingTotal)}
              note={missingTotal > missing.length ? `first ${missing.length} listed below` : "Listed below for audit"}
            />
          </section>
          <div className="coverage-grid">
            <section className="panel">
              <div className="panel-head">
                <div>
                  <p className="eyebrow">By instrument</p>
                  <h2>Selected-session coverage</h2>
                </div>
              </div>
              <div className="table-wrap">
                <table>
                  <thead>
                    <tr>
                      <th>Symbol</th>
                      <th>Covered</th>
                      <th>Total</th>
                      <th>Coverage</th>
                    </tr>
                  </thead>
                  <tbody>
                    {bySymbol.slice(0, SYMBOL_TABLE_CAP).map(([symbol, value]) => (
                      <tr key={symbol}>
                        <td>{symbol}</td>
                        <td>{value.covered}</td>
                        <td>{value.total}</td>
                        <td>
                          {number((100 * value.covered) / value.total, 2)}%
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            </section>
            <section className="panel">
              <div className="panel-head">
                <div>
                  <p className="eyebrow">By year</p>
                  <h2>Coverage stability</h2>
                </div>
              </div>
              <div className="table-wrap">
                <table>
                  <thead>
                    <tr>
                      <th>Year</th>
                      <th>Covered</th>
                      <th>Total</th>
                      <th>Coverage</th>
                    </tr>
                  </thead>
                  <tbody>
                    {byYear.map(([year, value]) => (
                      <tr key={year}>
                        <td>{year}</td>
                        <td>{value.covered}</td>
                        <td>{value.total}</td>
                        <td>
                          {number((100 * value.covered) / value.total, 2)}%
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            </section>
          </div>
          <section className="panel">
            <div className="panel-head">
              <div>
                <p className="eyebrow">Exact exceptions</p>
                <h2>Missing symbol-dates</h2>
              </div>
              <span className="market-date">{missingTotal} ROWS{missingTotal > missing.length ? ` · FIRST ${missing.length}` : ""}</span>
            </div>
            <div className="table-wrap coverage-exceptions">
              <table>
                <thead>
                  <tr>
                    <th>Date</th>
                    <th>Symbol</th>
                    <th>Status</th>
                  </tr>
                </thead>
                <tbody>
                  {missing.map((row, index) => (
                    <tr key={`${row.trade_date}-${row.symbol}-${index}`}>
                      <td>{row.trade_date}</td>
                      <td>{row.symbol}</td>
                      <td>{row.status.replace("_", " ")}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          </section>
        </>
      )}
    </>
  );
}


function parseSweepValues(value: string) {
  return [...new Set(value.split(",").map((item) => Number(item.trim())))]
    .filter(Number.isFinite)
    .slice(0, 5);
}

function MultiRunChart({ details }: { details: RunDetail[] }) {
  const series = useMemo(() => {
    const usable = details.filter((detail) => detail.report?.daily.length);
    if (!usable.length) return [];
    const all = usable.flatMap((detail) => {
      const daily = detail.report!.daily;
      const base = Math.max(daily[0].equity, 0.01);
      return daily.map((row) => Math.log(Math.max(row.equity / base, 0.01)));
    });
    const min = Math.min(...all);
    const max = Math.max(...all);
    const span = Math.max(max - min, 0.0001);
    return usable.map((detail, colorIndex) => {
      const daily = detail.report!.daily;
      const base = Math.max(daily[0].equity, 0.01);
      const points = daily
        .map((row, index) => {
          const value = Math.log(Math.max(row.equity / base, 0.01));
          const x = 36 + (index / Math.max(daily.length - 1, 1)) * 728;
          const y = 204 - ((value - min) / span) * 168;
          return `${x},${y}`;
        })
        .join(" ");
      return {
        id: detail.run.id,
        name: detail.run.name,
        color: comparisonPalette[colorIndex],
        points,
      };
    });
  }, [details]);
  if (!series.length)
    return <div className="empty-state">Select completed runs to compare.</div>;
  return (
    <div className="comparison-chart">
      <div className="chart-legend">
        {series.map((item) => (
          <span key={item.id}>
            <i style={{ background: item.color }} /> {item.name}
          </span>
        ))}
      </div>
      <svg viewBox="0 0 800 232" role="img" aria-label="Normalized log equity comparison">
        {[36, 78, 120, 162, 204].map((y) => (
          <line key={y} x1="36" y1={y} x2="764" y2={y} className="chart-grid" />
        ))}
        {series.map((item) => (
          <polyline
            key={item.id}
            points={item.points}
            fill="none"
            stroke={item.color}
            strokeWidth="2"
            vectorEffect="non-scaling-stroke"
          />
        ))}
      </svg>
    </div>
  );
}

type SweepOption = { value: string; label: string; suggested: string };

/** Numeric manifest parameters become sweep axes, with a 3-point grid around the default. */
function sweepOptionsFromManifest(manifest: SdkManifest | null): SweepOption[] {
  if (!manifest) return [];
  return manifest.params
    .filter((param) => param.kind === "int" || param.kind === "decimal")
    .map((param) => {
      const base = typeof param.default === "number" ? param.default : 0;
      const isInt = param.kind === "int";
      const clamp = (value: number) => {
        let next = value;
        if (param.min != null) next = Math.max(param.min, next);
        if (param.max != null) next = Math.min(param.max, next);
        return isInt ? Math.round(next) : Number(next.toPrecision(4));
      };
      const grid = base === 0
        ? [0, isInt ? 1 : 0.5, isInt ? 2 : 1]
        : [clamp(base * 0.5), clamp(base), clamp(base * 1.5)];
      const values = Array.from(new Set(grid)).join(",");
      const unit = param.unit ? ` (${param.unit})` : "";
      return { value: param.name, label: `${param.label || param.name}${unit}`, suggested: values };
    });
}

function CompareWorkspace({
  strategies,
  runs,
  sweeps,
  detail,
  compareDetails,
  busy,
  onCreate,
  onSelectSweep,
  onToggleRun,
  onOpenRun,
}: {
  strategies: Strategy[];
  runs: Run[];
  sweeps: SweepRecord[];
  detail: SweepDetail | null;
  compareDetails: RunDetail[];
  busy: boolean;
  onCreate: (request: Record<string, unknown>) => void;
  onSelectSweep: (id: string) => void;
  onToggleRun: (id: string) => void;
  onOpenRun: (id: string) => void;
}) {
  const runnable = strategies.filter(
    (strategy) =>
      strategy.runnable &&
      strategy.base_strategy_id === "sdk" &&
      !strategy.id.endsWith("__dev"),
  );
  const [strategyId, setStrategyId] = useState(runnable[0]?.id ?? "");
  const [sweepOptions, setSweepOptions] = useState<SweepOption[]>([]);
  const [axisOne, setAxisOne] = useState("");
  const [axisOneValues, setAxisOneValues] = useState("");
  const [axisTwo, setAxisTwo] = useState("");
  const [axisTwoValues, setAxisTwoValues] = useState("");
  const [metric, setMetric] = useState<"sharpe" | "cagr" | "drawdown">("sharpe");

  // Sweepable axes come from the strategy's manifest: every numeric parameter.
  useEffect(() => {
    if (!strategyId) return;
    let cancelled = false;
    fetch(`${API}/strategies/${strategyId}`, { cache: "no-store" })
      .then((response) => (response.ok ? response.json() : null))
      .then((body: StrategyDetail | null) => {
        if (cancelled) return;
        const options = sweepOptionsFromManifest(body?.sdk ?? null);
        setSweepOptions(options);
        setAxisOne(options[0]?.value ?? "");
        setAxisOneValues(options[0]?.suggested ?? "");
        setAxisTwo("");
        setAxisTwoValues(options[1]?.suggested ?? "");
      })
      .catch(() => undefined);
    return () => {
      cancelled = true;
    };
  }, [strategyId]);

  function changeStrategy(id: string) {
    setStrategyId(id);
  }
  function changeAxisTwo(name: string) {
    setAxisTwo(name);
    const option = sweepOptions.find((candidate) => candidate.value === name);
    if (option) setAxisTwoValues(option.suggested);
  }
  function changeAxisOne(name: string) {
    setAxisOne(name);
    const option = sweepOptions.find((candidate) => candidate.value === name);
    if (option) setAxisOneValues(option.suggested);
  }
  const axisOneParsed = parseSweepValues(axisOneValues);
  const axisTwoParsed = axisTwo ? parseSweepValues(axisTwoValues) : [];
  const count = axisOneParsed.length * Math.max(axisTwoParsed.length, 1);
  const completedRuns = runs.filter(
    (run) => !run.legacy && run.status === "Complete",
  );
  const selectedIds = new Set(compareDetails.map((item) => item.run.id));
  const ranked = [...(detail?.members ?? [])]
    .filter((member) => member.metrics)
    .sort((a, b) => {
      const score = (member: SweepMember) => {
        const m = member.metrics!;
        return (
          (m.sharpe ?? -20) -
          Math.max(0, Math.abs(m.max_drawdown_percent) - 30) / 20 -
          (m.trade_count < 30 ? 0.5 : 0)
        );
      };
      return score(b) - score(a);
    });
  const heatValue = (member?: SweepMember) => {
    if (!member?.metrics) return undefined;
    if (metric === "cagr") return member.metrics.cagr_percent;
    if (metric === "drawdown") return member.metrics.max_drawdown_percent;
    return member.metrics.sharpe;
  };
  return (
    <div className="compare-workspace">
      <section className="section-intro compare-intro">
        <div>
          <p className="eyebrow">Controlled research</p>
          <h2>Sweeps & comparisons</h2>
          <p>
            Grid search is confined to Development data. Every configuration becomes an
            immutable standard run; validation and final holdout remain untouched.
          </p>
        </div>
        <span className="discipline-badge">DEV RANKING ONLY</span>
      </section>

      <section className="panel sweep-builder">
        <div className="terminal-panel-title"><span>SWP</span> NEW PARAMETER GRID</div>
        <form
          onSubmit={(event) => {
            event.preventDefault();
            const form = new FormData(event.currentTarget);
            const axes: SweepAxis[] = [
              { parameter: axisOne, values: axisOneParsed },
            ];
            if (axisTwo) axes.push({ parameter: axisTwo, values: axisTwoParsed });
            onCreate({
              strategy_id: strategyId,
              name: form.get("name"),
              start_date: form.get("start_date"),
              end_date: form.get("end_date"),
              base_parameters: {},
              axes,
              costs_enabled: form.get("costs_enabled") === "on",
            });
          }}
        >
          <div className="sweep-form-grid">
            <label>
              Strategy
              <select value={strategyId} onChange={(e) => changeStrategy(e.target.value)}>
                {runnable.map((strategy) => (
                  <option key={strategy.id} value={strategy.id}>{strategy.name}</option>
                ))}
              </select>
            </label>
            <label>
              Sweep name
              <input name="name" defaultValue="Development sensitivity" maxLength={80} required />
            </label>
            <label>
              Development start
              <input name="start_date" type="date" defaultValue="2020-01-01" required />
            </label>
            <label>
              Development end
              <input name="end_date" type="date" defaultValue="2023-12-31" required />
            </label>
            <label>
              X parameter
              <select value={axisOne} onChange={(e) => changeAxisOne(e.target.value)}>
                {sweepOptions.map((option) => (
                  <option key={option.value} value={option.value}>{option.label}</option>
                ))}
              </select>
            </label>
            <label>
              X values
              <input value={axisOneValues} onChange={(e) => setAxisOneValues(e.target.value)} />
              <small>2–5 comma-separated values</small>
            </label>
            <label>
              Y parameter
              <select value={axisTwo} onChange={(e) => changeAxisTwo(e.target.value)}>
                <option value="">One-dimensional sweep</option>
                {sweepOptions
                  .filter((option) => option.value !== axisOne)
                  .map((option) => (
                    <option key={option.value} value={option.value}>{option.label}</option>
                  ))}
              </select>
            </label>
            <label>
              Y values
              <input
                value={axisTwoValues}
                disabled={!axisTwo}
                onChange={(e) => setAxisTwoValues(e.target.value)}
              />
              <small>2–5 comma-separated values</small>
            </label>
          </div>
          <div className="sweep-submit">
            <label className="cost-check">
              <input name="costs_enabled" type="checkbox" defaultChecked />
              Net of configured costs
            </label>
            <span>{count || 0} immutable configurations · 2 workers</span>
            <button className="primary-action" disabled={busy || !axisOne || count < 2 || count > 25}>
              Queue controlled sweep →
            </button>
          </div>
        </form>
      </section>

      <div className="compare-grid">
        <section className="panel sweep-history">
          <div className="terminal-panel-title"><span>HST</span> SWEEP HISTORY</div>
          {sweeps.length ? sweeps.map((sweep) => (
            <button
              key={sweep.id}
              className={detail?.sweep.id === sweep.id ? "sweep-row active" : "sweep-row"}
              onClick={() => onSelectSweep(sweep.id)}
            >
              <strong>{sweep.name}</strong>
              <span>{sweep.complete_count}/{sweep.configuration_count}</span>
              <em>{sweep.status}</em>
            </button>
          )) : <div className="empty-state">No controlled sweeps yet.</div>}
        </section>
        <section className="panel sweep-results">
          <div className="terminal-panel-title">
            <span>MAP</span> SENSITIVITY
            <select value={metric} onChange={(e) => setMetric(e.target.value as typeof metric)}>
              <option value="sharpe">Sharpe</option>
              <option value="cagr">CAGR %</option>
              <option value="drawdown">Max DD %</option>
            </select>
          </div>
          {!detail ? <div className="empty-state">Select a sweep to inspect.</div> : (
            <>
              <div className="sweep-summary-strip">
                <strong>{detail.sweep.name}</strong>
                <span>{detail.sweep.start_date} → {detail.sweep.end_date}</span>
                <span>{detail.sweep.complete_count}/{detail.sweep.configuration_count} complete</span>
              </div>
              {detail.sweep.axes.length === 2 ? (
                <div className="table-wrap sweep-heatmap"><table>
                  <thead><tr><th>{detail.sweep.axes[1].parameter} ↓ / {detail.sweep.axes[0].parameter} →</th>
                    {detail.sweep.axes[0].values.map((value) => <th key={value}>{value}</th>)}
                  </tr></thead>
                  <tbody>{detail.sweep.axes[1].values.map((y) => (
                    <tr key={y}><th>{y}</th>
                      {detail.sweep.axes[0].values.map((x) => {
                        const member = detail.members.find((candidate) =>
                          candidate.parameters[detail.sweep.axes[0].parameter] === x &&
                          candidate.parameters[detail.sweep.axes[1].parameter] === y,
                        );
                        const value = heatValue(member);
                        return <td key={x} className={value == null ? "pending-cell" : value >= 0 ? "heat-positive" : "heat-negative"}>
                          {value == null ? "·" : number(value, 2)}
                        </td>;
                      })}
                    </tr>
                  ))}</tbody>
                </table></div>
              ) : (
                <div className="table-wrap"><table><thead><tr><th>Value</th><th>Sharpe</th><th>CAGR</th><th>Max DD</th></tr></thead>
                  <tbody>{detail.members.map((member) => <tr key={member.run_id}>
                    <td>{member.parameters[detail.sweep.axes[0].parameter]}</td>
                    <td>{number(member.metrics?.sharpe)}</td>
                    <td>{percent(member.metrics?.cagr_percent)}</td>
                    <td>{percent(member.metrics?.max_drawdown_percent)}</td>
                  </tr>)}</tbody>
                </table></div>
              )}
            </>
          )}
        </section>
      </div>

      {detail && <section className="panel ranked-configs">
        <div className="terminal-panel-title"><span>RNK</span> DEVELOPMENT RANKING</div>
        <div className="table-wrap"><table><thead><tr><th>#</th><th>Parameters</th><th>Sharpe</th><th>CAGR</th><th>Max DD</th><th>Trades</th><th>Evidence</th></tr></thead>
          <tbody>{ranked.map((member, index) => <tr key={member.run_id}>
            <td>{index + 1}</td>
            <td>{detail.sweep.axes.map((axis) => `${axis.parameter}=${member.parameters[axis.parameter]}`).join(" · ")}</td>
            <td>{number(member.metrics?.sharpe)}</td>
            <td className={classFor(member.metrics?.cagr_percent)}>{percent(member.metrics?.cagr_percent)}</td>
            <td className="negative-text">{percent(member.metrics?.max_drawdown_percent)}</td>
            <td>{member.metrics?.trade_count}</td>
            <td><button className="text-action" onClick={() => onOpenRun(member.run_id)}>Open run →</button></td>
          </tr>)}</tbody>
        </table></div>
        <p className="ranking-note">Rank applies a small penalty below 30 trades and beyond 30% drawdown; it never uses validation or holdout results.</p>
      </section>}

      <section className="panel run-comparison">
        <div className="terminal-panel-title"><span>CMP</span> RUN COMPARISON <small>SELECT UP TO FOUR</small></div>
        <div className="comparison-picker">
          {completedRuns.slice(0, 40).map((run) => (
            <label key={run.id} className={selectedIds.has(run.id) ? "selected" : ""}>
              <input
                type="checkbox"
                checked={selectedIds.has(run.id)}
                disabled={!selectedIds.has(run.id) && selectedIds.size >= 4}
                onChange={() => onToggleRun(run.id)}
              />
              <span>{run.name}</span>
              <small>{run.research_label}</small>
            </label>
          ))}
        </div>
        <MultiRunChart details={compareDetails} />
        {compareDetails.length > 0 && <div className="table-wrap comparison-metrics"><table>
          <thead><tr><th>Run</th><th>CAGR</th><th>Sharpe</th><th>Sortino</th><th>Max DD</th><th>Vol</th><th>Trades</th></tr></thead>
          <tbody>{compareDetails.map((item) => <tr key={item.run.id}>
            <td>{item.run.name}</td>
            <td className={classFor(item.report?.metrics.cagr_percent)}>{percent(item.report?.metrics.cagr_percent)}</td>
            <td>{number(item.report?.metrics.sharpe)}</td>
            <td>{number(item.report?.metrics.sortino)}</td>
            <td className="negative-text">{percent(item.report?.metrics.max_drawdown_percent)}</td>
            <td>{percent(item.report?.metrics.annual_volatility_percent)}</td>
            <td>{item.report?.trades.length ?? "—"}</td>
          </tr>)}</tbody>
        </table></div>}
      </section>
    </div>
  );
}

function PortfolioWorkspace({
  runs,
  portfolios,
  detail,
  seedRunIds,
  busy,
  onCreate,
  onSelect,
}: {
  runs: Run[];
  portfolios: PortfolioRecord[];
  detail: RunDetail | null;
  seedRunIds: string[];
  busy: boolean;
  onCreate: (request: Record<string, unknown>) => void;
  onSelect: (runId: string) => void;
}) {
  const [mode, setMode] = useState("unconstrained_overlays");
  const [selected, setSelected] = useState<Record<string, number>>(() =>
    Object.fromEntries(seedRunIds.slice(0, 8).map((id) => [id, 1])),
  );
  const [showAll, setShowAll] = useState(false);
  const usableRuns = runs.filter(
    (run) =>
      !run.legacy &&
      run.status === "Complete" &&
      run.research_label !== "Portfolio" &&
      (showAll || run.starred),
  );
  const selectedIds = Object.keys(selected);
  function toggleRun(id: string) {
    setSelected((current) => {
      if (id in current) {
        const next = { ...current };
        delete next[id];
        return next;
      }
      if (Object.keys(current).length >= 8) return current;
      return { ...current, [id]: 1 };
    });
  }
  const modes = [
    {
      id: "unconstrained_overlays",
      code: "OVR",
      title: "Full sleeves / unconstrained",
      copy: "Every strategy keeps its full standalone sizing. Concurrent sleeves are additive and gross capital may exceed 100%.",
    },
    {
      id: "sequential_full_capital",
      code: "SEQ",
      title: "Reuse capital when non-overlapping",
      copy: "The engine orders entries and exits and compounds then-available capital. Any overlap between strategies is rejected.",
    },
    {
      id: "normalized_weights",
      code: "WGT",
      title: "Fixed allocation",
      copy: "Weights are normalized to 100% and rebalanced daily over the shared source calendar.",
    },
  ];
  return (
    <div className="portfolio-workspace">
      <section className="section-intro compare-intro">
        <div>
          <p className="eyebrow">Capital timeline</p>
          <h2>Portfolio Builder</h2>
          <p>
            Combine frozen strategy runs without changing their internal volatility targets,
            leverage caps, or transaction costs.
          </p>
        </div>
        <span className="discipline-badge">EVENT-TIME ACCOUNTING</span>
      </section>

      <section className="panel portfolio-builder">
        <div className="terminal-panel-title"><span>PORT</span> CONSTRUCTION METHOD</div>
        <div className="capital-mode-grid">
          {modes.map((item) => (
            <label key={item.id} className={mode === item.id ? "capital-mode active" : "capital-mode"}>
              <input
                type="radio"
                name="capital_mode"
                value={item.id}
                checked={mode === item.id}
                onChange={() => setMode(item.id)}
              />
              <span>{item.code}</span>
              <strong>{item.title}</strong>
              <small>{item.copy}</small>
            </label>
          ))}
        </div>
        <form
          onSubmit={(event) => {
            event.preventDefault();
            const form = new FormData(event.currentTarget);
            onCreate({
              name: form.get("name"),
              initial_capital: Number(form.get("initial_capital")),
              capital_mode: mode,
              components: selectedIds.map((runId) => ({
                run_id: runId,
                weight: selected[runId],
                capital_group: mode === "unconstrained_overlays" ? "overlay" : null,
              })),
            });
          }}
        >
          <div className="portfolio-fields">
            <label>
              Portfolio name
              <input name="name" defaultValue="Strategy portfolio" required maxLength={100} />
            </label>
            <label>
              Initial capital
              <input name="initial_capital" type="number" defaultValue="100000" min="1000" step="1000" required />
            </label>
            <div className="portfolio-assumption">
              <span>COSTS</span>
              <strong>Embedded in sources</strong>
              <small>No second transaction-cost charge</small>
            </div>
            <div className="portfolio-assumption">
              <span>VOL</span>
              <strong>Source targets retained</strong>
              <small>No look-ahead portfolio rescaling</small>
            </div>
          </div>
          <div className="portfolio-source-grid">
            <div className="source-picker">
              <div className="terminal-panel-title">
                <span>SRC</span> {showAll ? "COMPLETED RUNS" : "STARRED RUNS"}{" "}
                <small>{selectedIds.length}/8 SELECTED</small>
                <button
                  type="button"
                  className="text-action source-filter"
                  onClick={() => setShowAll((value) => !value)}
                >
                  {showAll ? "★ Starred only" : "Show all"}
                </button>
              </div>
              {usableRuns.map((run) => (
                <label key={run.id} className={run.id in selected ? "selected" : ""}>
                  <input
                    type="checkbox"
                    checked={run.id in selected}
                    disabled={!(run.id in selected) && selectedIds.length >= 8}
                    onChange={() => toggleRun(run.id)}
                  />
                  <span>
                    <strong>{run.starred ? "★ " : ""}{run.name}</strong>
                    <small>{run.research_label} · {run.start_date} → {run.end_date}</small>
                    <small className="run-metrics-line">{metricsLine(run)}</small>
                  </span>
                </label>
              ))}
              {!usableRuns.length && (
                <div className="empty-state">
                  {showAll
                    ? "Run at least two UI backtests first."
                    : "No starred runs yet. Star runs on a strategy page or in the Runs list to make them available here."}
                </div>
              )}
            </div>
            <div className="allocation-panel">
              <div className="terminal-panel-title"><span>ALOC</span> SLEEVE WEIGHTS</div>
              {selectedIds.map((id) => {
                const run = usableRuns.find((item) => item.id === id);
                return <label key={id}>
                  <span>{run?.name ?? id}</span>
                  <input
                    type="number"
                    min="0.01"
                    max={mode === "normalized_weights" ? "100" : "1"}
                    step="0.05"
                    value={selected[id]}
                    onChange={(event) => setSelected((current) => ({ ...current, [id]: Number(event.target.value) }))}
                  />
                  <small>{mode === "normalized_weights" ? "relative weight" : "capital multiple"}</small>
                </label>;
              })}
              {!selectedIds.length && <div className="empty-state">Choose source runs.</div>}
            </div>
          </div>
          <div className="sweep-submit portfolio-submit">
            <span>
              {mode === "unconstrained_overlays"
                ? `${selectedIds.length}× full-capital sleeves; financing remains unmodeled.`
                : "The engine will verify the selected capital constraint."}
            </span>
            <button className="primary-action" disabled={busy || selectedIds.length < 2}>
              {busy ? "Building…" : "Build immutable portfolio →"}
            </button>
          </div>
        </form>
      </section>

      <section className="panel portfolio-history">
        <div className="terminal-panel-title"><span>HST</span> SAVED PORTFOLIOS</div>
        {portfolios.length ? portfolios.map((portfolio) => (
          <button key={portfolio.id} onClick={() => onSelect(portfolio.run_id)}>
            <strong>{portfolio.name}</strong>
            <span>{portfolio.component_count} sleeves</span>
            <span>{portfolio.capital_mode.replaceAll("_", " ")}</span>
            <time>{new Date(portfolio.created_at).toLocaleDateString()}</time>
          </button>
        )) : <div className="empty-state">No saved portfolios yet.</div>}
      </section>

      {detail && (
        <section className="portfolio-result">
          <div className="terminal-panel-title"><span>RSLT</span> {detail.run.name.toUpperCase()}</div>
          <RunReport detail={detail} />
        </section>
      )}
    </div>
  );
}

function CostsWorkspace({
  profiles,
  busy,
  onCreate,
}: {
  profiles: CostProfile[];
  busy: boolean;
  onCreate: (request: Record<string, unknown>) => void;
}) {
  return (
    <div className="costs-workspace">
      <section className="strategy-hero">
        <div>
          <p className="eyebrow">Central assumption library</p>
          <h2>Execution cost profiles</h2>
          <p>
            Profiles are versioned, write-once assumptions. Every queued run
            freezes the selected profile inside its manifest, while explicit
            strategy parameters remain auditable overrides.
          </p>
        </div>
        <span className="discipline-badge">ENTRY + EXIT STORED SEPARATELY</span>
      </section>
      <section className="panel cost-profile-grid">
        {profiles.map((profile) => (
          <article className="cost-profile-card" key={profile.id}>
            <div className="terminal-panel-title">
              <span>{profile.builtin ? "BASE" : "USER"}</span> {profile.asset_class.toUpperCase()}
            </div>
            <h3>{profile.name}</h3>
            <strong>{profile.model.replaceAll("_", " ")}</strong>
            {profile.model === "all_in_bps" ? (
              <dl>
                <div><dt>Entry</dt><dd>{profile.entry_bps.toFixed(2)} bps</dd></div>
                <div><dt>Exit</dt><dd>{profile.exit_bps.toFixed(2)} bps</dd></div>
                <div><dt>Round trip</dt><dd>{(profile.entry_bps + profile.exit_bps).toFixed(2)} bps</dd></div>
              </dl>
            ) : profile.model === "fixed_tick_per_unit" ? (
              <dl>
                <div><dt>Slippage</dt><dd>{profile.entry_slippage_ticks} / {profile.exit_slippage_ticks} ticks</dd></div>
                <div><dt>Commission</dt><dd>${profile.entry_commission_per_unit.toFixed(4)} / ${profile.exit_commission_per_unit.toFixed(4)}</dd></div>
                <div><dt>Tick size</dt><dd>${profile.tick_size}</dd></div>
              </dl>
            ) : <p>Zero commissions, spread, and slippage.</p>}
            <small>{profile.id}</small>
          </article>
        ))}
      </section>
      <section className="panel config-panel">
        <div className="panel-head"><div><p className="eyebrow">Write-once version</p><h2>Create cost profile</h2></div></div>
        <form onSubmit={(event) => {
          event.preventDefault();
          const form = new FormData(event.currentTarget);
          onCreate({
            name: form.get("name"), asset_class: form.get("asset_class"), model: form.get("model"),
            entry_bps: Number(form.get("entry_bps")), exit_bps: Number(form.get("exit_bps")),
            tick_size: Number(form.get("tick_size")), entry_slippage_ticks: Number(form.get("entry_slippage_ticks")),
            exit_slippage_ticks: Number(form.get("exit_slippage_ticks")),
            entry_commission_per_unit: Number(form.get("entry_commission_per_unit")),
            exit_commission_per_unit: Number(form.get("exit_commission_per_unit")),
            minimum_commission: Number(form.get("minimum_commission")),
          });
        }}>
          <div className="field-grid four">
            <label>Profile name<input name="name" required maxLength={100} placeholder="US equities · conservative" /></label>
            <label>Asset class<select name="asset_class"><option>US equities</option><option>US futures</option><option>Spot FX</option><option>Crypto spot</option><option>Any</option></select></label>
            <label>Model<select name="model"><option value="all_in_bps">All-in basis points</option><option value="fixed_tick_per_unit">Fixed tick + per unit</option><option value="none">Costs off</option></select></label>
            <label>Tick size<input name="tick_size" type="number" step="0.0001" defaultValue="0.01" /></label>
            <label>Entry bps<input name="entry_bps" type="number" step="0.1" min="0" defaultValue="5" /></label>
            <label>Exit bps<input name="exit_bps" type="number" step="0.1" min="0" defaultValue="5" /></label>
            <label>Entry slippage ticks<input name="entry_slippage_ticks" type="number" min="0" defaultValue="0" /></label>
            <label>Exit slippage ticks<input name="exit_slippage_ticks" type="number" min="0" defaultValue="0" /></label>
            <label>Entry commission / unit<input name="entry_commission_per_unit" type="number" step="0.001" min="0" defaultValue="0" /></label>
            <label>Exit commission / unit<input name="exit_commission_per_unit" type="number" step="0.001" min="0" defaultValue="0" /></label>
            <label>Minimum commission<input name="minimum_commission" type="number" step="0.01" min="0" defaultValue="0" /></label>
          </div>
          <div className="run-submit"><p>Existing profiles are never edited; create a new version when assumptions change.</p><button className="primary-action" disabled={busy}>{busy ? "Saving…" : "Save immutable profile →"}</button></div>
        </form>
      </section>
    </div>
  );
}

function AutomationsWorkspace({
  schedules,
  busy,
  onToggle,
  onRun,
  onCreate,
}: {
  schedules: AutomationSchedule[];
  busy: boolean;
  onToggle: (id: string) => void;
  onRun: (id: string) => void;
  onCreate: (request: Record<string, unknown>) => void;
}) {
  return (
    <section className="panel automation-panel">
      <div className="panel-head"><div><p className="eyebrow">America/Los_Angeles</p><h2>Data schedules</h2></div><span className="discipline-badge">LOCAL SERVICE MUST BE RUNNING</span></div>
      <div className="automation-list">
        {schedules.map((schedule) => (
          <article key={schedule.id}>
            <span className={schedule.enabled ? "status ready" : "status"}>{schedule.enabled ? "Active" : "Paused"}</span>
            <div><strong>{schedule.name}</strong><small>{schedule.weekdays} · {schedule.local_time} PT · {schedule.kind.replaceAll("_", " ")}</small><em>{schedule.last_status ?? "Never run"}</em></div>
            <button onClick={() => onRun(schedule.id)} disabled={busy}>Run now</button>
            <button onClick={() => onToggle(schedule.id)} disabled={busy}>{schedule.enabled ? "Pause" : "Enable"}</button>
          </article>
        ))}
      </div>
      <form className="automation-create" onSubmit={(event) => {
        event.preventDefault();
        const form = new FormData(event.currentTarget);
        onCreate({ name: form.get("name"), kind: form.get("kind"), local_time: form.get("local_time"), weekdays: form.get("weekdays"), enabled: form.get("enabled") === "on" });
      }}>
        <input name="name" required placeholder="Schedule name" />
        <select name="kind"><option value="data_update">Data update command</option></select>
        <input name="local_time" type="time" defaultValue="20:15" required />
        <input name="weekdays" defaultValue="mon,tue,wed,thu,fri" required />
        <label className="toggle-label"><input name="enabled" type="checkbox" /><span>Enable immediately</span></label>
        <button className="secondary-action" disabled={busy}>Add schedule</button>
      </form>
    </section>
  );
}

function CodeWorkspace({
  strategies,
  drafts,
  source,
  draftDetail,
  busy,
  onSelectStrategy,
  onSelectDraft,
  onCreateDraft,
  onSaveFile,
  onValidate,
  onRelease,
  onBuild,
}: {
  strategies: Strategy[];
  drafts: StrategyDraft[];
  source: StrategySource | null;
  draftDetail: StrategyDraftDetail | null;
  busy: boolean;
  onSelectStrategy: (id: string) => void;
  onSelectDraft: (id: string) => void;
  onCreateDraft: (request: Record<string, unknown>) => void;
  onSaveFile: (draftId: string, path: string, content: string) => void;
  onValidate: (draftId: string, action: string) => void;
  onRelease: (draftId: string, request: Record<string, unknown>) => void;
  onBuild: (draftId: string) => void;
}) {
  const files = draftDetail?.files ?? source?.files ?? [];
  const [selectedPath, setSelectedPath] = useState(files[0]?.path ?? "");
  const [codeText, setCodeText] = useState(files[0]?.content ?? "");
  const [dirty, setDirty] = useState(false);
  function chooseFile(path: string) {
    const file = files.find((candidate) => candidate.path === path);
    setSelectedPath(path);
    setCodeText(file?.content ?? "");
    setDirty(false);
  }
  const validation = draftDetail?.validation;
  const validationActive = ["queued", "running"].includes(
    validation?.status ?? "",
  );
  const lines = Math.max(codeText.split("\n").length, 1);
  return (
    <>
      <section className="strategy-hero code-hero">
        <div>
          <p className="eyebrow">Versioned strategy development</p>
          <h2>Strategy code workspace</h2>
          <p>
            Inspect exact implementations and imported reference source. Rust
            strategies can be forked, validated in an isolated build, and
            released as immutable runnable versions.
          </p>
        </div>
        <div className="strategy-badges">
          <span>Source hashes</span>
          <span>Background builds</span>
          <span>Immutable releases</span>
        </div>
      </section>
      <div className="code-workspace">
        <aside className="panel code-navigator">
          <div className="terminal-panel-title">
            <span>SRC</span> RELEASED STRATEGIES
          </div>
          <div className="code-nav-list">
            {strategies.map((strategy) => (
              <button
                key={strategy.id}
                className={source?.strategy.id === strategy.id ? "active" : ""}
                onClick={() => onSelectStrategy(strategy.id)}
              >
                <strong>{strategy.name}</strong>
                <small>
                  {strategy.version} · {strategy.custom ? "custom" : "built-in"}
                </small>
              </button>
            ))}
          </div>
          <div className="terminal-panel-title draft-title">
            <span>DEV</span> EDITABLE DRAFTS
          </div>
          <div className="code-nav-list">
            {drafts.length ? (
              drafts.map((draft) => (
                <button
                  key={draft.id}
                  className={draftDetail?.draft.id === draft.id ? "active" : ""}
                  onClick={() => onSelectDraft(draft.id)}
                >
                  <strong>{draft.name}</strong>
                  <small>
                    {draft.version} · {draft.status}
                  </small>
                </button>
              ))
            ) : (
              <p className="code-empty">No drafts yet.</p>
            )}
          </div>
          <form
            className="new-draft-form sdk-new-form"
            onSubmit={(event) => {
              event.preventDefault();
              const form = new FormData(event.currentTarget);
              onCreateDraft({
                base_strategy_id: "sdk",
                strategy_id: form.get("strategy_id"),
                name: form.get("name"),
                version: "draft v1",
                description: form.get("description") || "New one-file SDK strategy",
              });
              event.currentTarget.reset();
            }}
          >
            <h3>New one-file strategy</h3>
            <label>
              Strategy id
              <input
                name="strategy_id"
                required
                pattern="[a-z][a-z0-9_]{2,47}"
                placeholder="rsi_mean_reversion"
                title="snake_case: lowercase letters, digits, underscores"
              />
            </label>
            <label>
              Name
              <input name="name" required maxLength={100} placeholder="RSI Mean Reversion" />
            </label>
            <label>
              Description
              <textarea name="description" maxLength={300} rows={2} placeholder="What the edge is" />
            </label>
            <button className="primary-action" disabled={busy}>
              Create skeleton
            </button>
            <small>
              Writes src/strategies/user/&lt;id&gt;.rs from the SDK template. Parameters you declare in
              manifest() appear on the run form automatically.
            </small>
          </form>
          <form
            className="new-draft-form"
            onSubmit={(event) => {
              event.preventDefault();
              const form = new FormData(event.currentTarget);
              onCreateDraft({
                base_strategy_id: form.get("base_strategy_id"),
                name: form.get("name"),
                version: form.get("version"),
                description: form.get("description"),
              });
            }}
          >
            <h3>Fork an existing strategy</h3>
            <label>
              Starting template
              <select name="base_strategy_id" defaultValue={source?.strategy.id}>
                {strategies.filter((strategy) => strategy.runnable).map((strategy) => (
                  <option key={strategy.id} value={strategy.id}>
                    {strategy.name} · {strategy.version}
                  </option>
                ))}
              </select>
            </label>
            <label>
              Name
              <input name="name" required maxLength={100} placeholder="My strategy" />
            </label>
            <label>
              Draft version
              <input name="version" required maxLength={40} defaultValue="draft v1" />
            </label>
            <label>
              Description
              <textarea name="description" required maxLength={300} rows={3} placeholder="What this version changes" />
            </label>
            <button className="secondary-action" disabled={busy}>
              Create from template
            </button>
          </form>
        </aside>
        <section className="panel code-editor-panel">
          {(source || draftDetail) && files.length ? (
            <>
              <div className="code-editor-head">
                <div>
                  <p className="eyebrow">
                    {draftDetail ? "Editable draft" : "Immutable release"}
                  </p>
                  <h2>
                    {draftDetail?.draft.name ?? source?.strategy.name}{" "}
                    <span className="version">
                      {draftDetail?.draft.version ?? source?.strategy.version}
                    </span>
                  </h2>
                  <code>
                    SHA-256 {draftDetail?.draft.source_sha256 ?? source?.source_sha256}
                  </code>
                </div>
                {draftDetail && (
                  <span className={`status ${draftDetail.draft.status === "validated" ? "ready" : ""}`}>
                    {draftDetail.draft.status}
                  </span>
                )}
              </div>
              <div className="code-file-tabs">
                {files.map((file) => (
                  <button
                    key={file.path}
                    className={selectedPath === file.path ? "active" : ""}
                    onClick={() => chooseFile(file.path)}
                  >
                    {file.path}
                  </button>
                ))}
              </div>
              <div className="rust-editor">
                <pre aria-hidden="true" className="line-numbers">
                  {Array.from({ length: lines }, (_, index) => index + 1).join("\n")}
                </pre>
                <textarea
                  aria-label={`Strategy source ${selectedPath}`}
                  value={codeText}
                  readOnly={!draftDetail || draftDetail.draft.status === "released"}
                  spellCheck={false}
                  onChange={(event) => {
                    setCodeText(event.target.value);
                    setDirty(true);
                  }}
                  onKeyDown={(event) => {
                    if (event.key !== "Tab" || !draftDetail) return;
                    event.preventDefault();
                    const target = event.currentTarget;
                    const start = target.selectionStart;
                    const end = target.selectionEnd;
                    const next = `${codeText.slice(0, start)}    ${codeText.slice(end)}`;
                    setCodeText(next);
                    setDirty(true);
                    requestAnimationFrame(() => {
                      target.selectionStart = target.selectionEnd = start + 4;
                    });
                  }}
                />
              </div>
              {draftDetail && draftDetail.draft.status !== "released" && (
                <div className="code-actions">
                  <button
                    className="primary-action"
                    disabled={busy || !dirty}
                    onClick={() =>
                      onSaveFile(draftDetail.draft.id, selectedPath, codeText)
                    }
                  >
                    {dirty ? "Save file" : "Saved"}
                  </button>
                  <button disabled={busy || dirty || validationActive} onClick={() => onValidate(draftDetail.draft.id, "format")}>Format</button>
                  <button disabled={busy || dirty || validationActive} onClick={() => onValidate(draftDetail.draft.id, "check")}>Compile</button>
                  <button disabled={busy || dirty || validationActive} onClick={() => onValidate(draftDetail.draft.id, "test")}>Run tests</button>
                  {draftDetail.draft.base_strategy_id === "sdk" && (
                    <button
                      className="primary-action"
                      disabled={busy || dirty || validationActive}
                      onClick={() => onBuild(draftDetail.draft.id)}
                    >
                      Build &amp; run
                    </button>
                  )}
                  <small>
                    {draftDetail.draft.base_strategy_id === "sdk"
                      ? "Save, then Build & run compiles a dev engine and opens the strategy page. Release freezes an immutable version."
                      : "Save before validating. Tests include format and compilation."}
                  </small>
                </div>
              )}
              {validation && (
                <section className="validation-console">
                  <div className="terminal-panel-title">
                    <span>{validation.action.toUpperCase()}</span> {validation.status.toUpperCase()}
                    <small>{validation.id}</small>
                  </div>
                  {validation.error && <p className="negative-text">{validation.error}</p>}
                  <pre>{validation.log || (validationActive ? "Waiting for worker output…" : "No output captured.")}</pre>
                </section>
              )}
              {draftDetail?.draft.status === "validated" && (
                <form
                  className="release-form"
                  onSubmit={(event) => {
                    event.preventDefault();
                    const form = new FormData(event.currentTarget);
                    onRelease(draftDetail.draft.id, {
                      strategy_id: form.get("strategy_id"),
                      name: form.get("name"),
                      version: form.get("version"),
                      description: form.get("description"),
                    });
                  }}
                >
                  <div>
                    <p className="eyebrow">Immutable promotion</p>
                    <h3>Release runnable strategy version</h3>
                    <p>This compiles a dedicated engine binary and registers a new catalog identity. It cannot overwrite an existing release.</p>
                  </div>
                  <input name="strategy_id" required pattern="[a-z][a-z0-9_]{2,79}" placeholder="my_strategy_v1" />
                  <input name="name" required maxLength={100} defaultValue={draftDetail.draft.name} />
                  <input name="version" required maxLength={40} defaultValue={draftDetail.draft.version.replace("draft", "v")} />
                  <textarea name="description" required maxLength={300} rows={2} defaultValue={draftDetail.draft.description} />
                  <button className="primary-action" disabled={busy}>Build immutable release</button>
                </form>
              )}
            </>
          ) : (
            <div className="empty-state">Select a released strategy or create a draft.</div>
          )}
        </section>
      </div>
    </>
  );
}

type CatalogGroupBy = "asset" | "status" | "none";
type CatalogSortKey = "name" | "version" | "status" | "asset" | "runs" | "last";
type CatalogRunStats = { count: number; complete: number; last?: string };
type CatalogRow = { strategy: Strategy; depth: number };
type CatalogGroup = { key: string; label: string; rows: CatalogRow[] };

const CATALOG_STATUS_RANK: Record<string, number> = {
  Production: 0,
  Research: 1,
  Sample: 2,
  "Data required": 3,
};

function catalogStatusClass(status: string) {
  const key = status.toLowerCase();
  if (key === "production") return "catalog-status production";
  if (key === "research") return "catalog-status research";
  if (key === "sample") return "catalog-status sample";
  return "catalog-status blocked";
}

function catalogSortValue(
  strategy: Strategy,
  key: CatalogSortKey,
  stats: Map<string, CatalogRunStats>,
): string | number {
  const runStats = stats.get(strategy.id);
  switch (key) {
    case "name":
      return strategy.name.toLowerCase();
    case "version":
      return strategy.version.toLowerCase();
    case "status":
      return CATALOG_STATUS_RANK[strategy.status] ?? 9;
    case "asset":
      return strategy.asset_scope.toLowerCase();
    case "runs":
      return runStats?.count ?? 0;
    case "last":
      return runStats?.last ?? "";
  }
}

/** Orders base strategies by the active sort and nests each custom release beneath its base. */
function orderCatalogRows(
  strategies: Strategy[],
  sortKey: CatalogSortKey,
  sortDir: "asc" | "desc",
  stats: Map<string, CatalogRunStats>,
): CatalogRow[] {
  const ids = new Set(strategies.map((strategy) => strategy.id));
  const compare = (a: Strategy, b: Strategy) => {
    const left = catalogSortValue(a, sortKey, stats);
    const right = catalogSortValue(b, sortKey, stats);
    const order = left < right ? -1 : left > right ? 1 : a.name.localeCompare(b.name);
    return sortDir === "asc" ? order : -order;
  };
  const roots = strategies
    .filter((strategy) => !strategy.base_strategy_id || !ids.has(strategy.base_strategy_id))
    .sort(compare);
  const children = new Map<string, Strategy[]>();
  for (const strategy of strategies) {
    if (strategy.base_strategy_id && ids.has(strategy.base_strategy_id)) {
      const list = children.get(strategy.base_strategy_id) ?? [];
      list.push(strategy);
      children.set(strategy.base_strategy_id, list);
    }
  }
  const rows: CatalogRow[] = [];
  const push = (strategy: Strategy, depth: number) => {
    rows.push({ strategy, depth });
    for (const child of (children.get(strategy.id) ?? []).sort(compare)) {
      push(child, depth + 1);
    }
  };
  roots.forEach((strategy) => push(strategy, 0));
  return rows;
}

const GAP_FADE_PRIORITY_ALIASES = ["IWM_MDY", "IWM+MDY", "IWM/MDY"];

/** Expands a stored US asset parameter ("IWM_MDY", "AAPL", "TLT,SPY") into full EODHD symbols. */
function usAssetListToSymbols(value: string): string[] {
  const upper = value.trim().toUpperCase();
  if (!upper) return [];
  if (GAP_FADE_PRIORITY_ALIASES.includes(upper)) return ["IWM.US", "MDY.US"];
  return Array.from(
    new Set(
      upper
        .split(/[,+/ _]+/)
        .filter(Boolean)
        .map((code) => (code.endsWith(".US") ? code : `${code}.US`)),
    ),
  );
}

function instrumentQueryParams(requirement: InstrumentRequirement) {
  const params = new URLSearchParams();
  if (requirement.suffixes.length) params.set("suffix", requirement.suffixes.join(","));
  if (requirement.resolutions.length) params.set("resolution", requirement.resolutions.join(","));
  if (requirement.asset_classes.length) params.set("asset_class", requirement.asset_classes.join(","));
  return params;
}

/** Instruments the run form currently targets, in the form the API validates. */
function selectedInstrumentSymbols(
  detail: StrategyDetail,
  parameters: Overrides,
): string[] | null {
  const requirement = detail.instruments;
  if (!requirement) return null;
  if (requirement.parameter === "symbols") {
    const fallback = detail.default_parameters.symbols;
    const chosen = parameters.symbols ?? (Array.isArray(fallback) ? (fallback as string[]) : []);
    return chosen;
  }
  const fallback =
    typeof detail.default_parameters.asset === "string"
      ? (detail.default_parameters.asset as string)
      : "";
  return usAssetListToSymbols(parameters.asset ?? fallback);
}

function InstrumentPicker({
  requirement,
  value,
  onChange,
  disabled,
}: {
  requirement: InstrumentRequirement;
  value: string[];
  onChange: (symbols: string[]) => void;
  disabled?: boolean;
}) {
  const [query, setQuery] = useState("");
  const [hits, setHits] = useState<InstrumentHit[]>([]);
  const [open, setOpen] = useState(false);
  const [cursor, setCursor] = useState(0);
  const [searching, setSearching] = useState(false);
  const [searchError, setSearchError] = useState("");
  const [resolved, setResolved] = useState<Record<string, InstrumentHit>>({});
  const baseParams = instrumentQueryParams(requirement).toString();
  const single = requirement.mode === "single";
  const full = !single && value.length >= requirement.maximum;
  const valueKey = value.join(",");

  // Resolve names and coverage for symbols that arrived from presets or defaults.
  useEffect(() => {
    const missing = valueKey.split(",").filter((symbol) => symbol && !resolved[symbol]);
    if (!missing.length) return;
    let cancelled = false;
    const params = new URLSearchParams(baseParams);
    params.set("symbols", missing.join(","));
    params.set("limit", String(missing.length));
    fetch(`${API}/instruments?${params}`, { cache: "no-store" })
      .then((response) => (response.ok ? response.json() : null))
      .then((body) => {
        if (cancelled) return;
        const found =
          (body as { instruments?: InstrumentHit[] } | null)?.instruments ?? [];
        setResolved((current) => {
          const next = { ...current };
          for (const hit of found) next[hit.symbol] = hit;
          for (const symbol of missing) {
            if (!next[symbol]) {
              const [code = symbol, suffix = ""] = symbol.split(".");
              next[symbol] = {
                symbol,
                code,
                suffix,
                name: "",
                exchange: suffix,
                asset_class: "Unknown",
                currency: "",
                status: "unknown",
                daily: false,
                five_minute: false,
                one_minute: false,
                coverage: {},
                missing_resolutions: requirement.resolutions,
              };
            }
          }
          return next;
        });
      })
      .catch(() => undefined);
    return () => {
      cancelled = true;
    };
  }, [valueKey, resolved, baseParams, requirement.resolutions]);

  // Debounced catalog search.
  useEffect(() => {
    const needle = query.trim();
    if (!needle) return;
    let cancelled = false;
    const timer = window.setTimeout(async () => {
      setSearching(true);
      try {
        const params = new URLSearchParams(baseParams);
        params.set("q", needle);
        params.set("limit", "12");
        const response = await fetch(`${API}/instruments?${params}`, { cache: "no-store" });
        const body = (await response.json()) as {
          error?: string;
          instruments?: InstrumentHit[];
        };
        if (!response.ok) throw new Error(body.error ?? "Instrument search failed");
        if (cancelled) return;
        setHits(body.instruments ?? []);
        setCursor(0);
        setOpen(true);
        setSearchError("");
      } catch (caught) {
        if (!cancelled) {
          setSearchError(caught instanceof Error ? caught.message : "Instrument search failed");
        }
      } finally {
        if (!cancelled) setSearching(false);
      }
    }, 160);
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
  }, [query, baseParams]);

  function add(hit: InstrumentHit) {
    if (disabled) return;
    setResolved((current) => ({ ...current, [hit.symbol]: hit }));
    if (single) onChange([hit.symbol]);
    else if (!value.includes(hit.symbol) && !full) onChange([...value, hit.symbol]);
    setQuery("");
    setHits([]);
    setOpen(false);
  }
  function remove(symbol: string) {
    onChange(value.filter((item) => item !== symbol));
  }
  function move(symbol: string, offset: number) {
    const index = value.indexOf(symbol);
    const target = index + offset;
    if (index < 0 || target < 0 || target >= value.length) return;
    const next = [...value];
    next.splice(index, 1);
    next.splice(target, 0, symbol);
    onChange(next);
  }
  function onKeyDown(event: KeyboardEvent<HTMLInputElement>) {
    if (event.key === "ArrowDown" && hits.length) {
      event.preventDefault();
      setOpen(true);
      setCursor((current) => Math.min(hits.length - 1, current + 1));
    } else if (event.key === "ArrowUp" && hits.length) {
      event.preventDefault();
      setCursor((current) => Math.max(0, current - 1));
    } else if (event.key === "Enter") {
      event.preventDefault();
      if (open && hits[cursor]) add(hits[cursor]);
    } else if (event.key === "Escape") {
      setOpen(false);
    } else if (event.key === "Backspace" && !query && value.length && !single) {
      remove(value[value.length - 1]);
    }
  }

  const coverageChips = (hit: InstrumentHit) =>
    (["daily", "5m", "1m"] as const).map((resolution) => {
      const present =
        resolution === "daily" ? hit.daily : resolution === "5m" ? hit.five_minute : hit.one_minute;
      const required = requirement.resolutions.includes(resolution);
      if (!present && !required) return null;
      const range = hit.coverage[resolution];
      return (
        <i
          key={resolution}
          className={
            present ? (required ? "cov required" : "cov") : "cov missing"
          }
          title={range ? `${range.first} → ${range.last}` : "no data on disk"}
        >
          {resolution.toUpperCase()}
          {range ? ` ${range.last.slice(2).replace(/-/g, "")}` : present ? "" : " ✕"}
        </i>
      );
    });

  return (
    <div className="instrument-picker">
      <div className="instrument-chips">
        {value.map((symbol, index) => {
          const hit = resolved[symbol];
          const warn = hit ? hit.missing_resolutions.length > 0 : false;
          return (
            <span className={warn ? "instrument-chip warn" : "instrument-chip"} key={symbol}>
              {!single && <b>{index + 1}</b>}
              <strong>{symbol}</strong>
              <small>{hit?.name || (hit?.status === "unknown" ? "not in catalog" : "")}</small>
              {hit && <span className="instrument-chip-cov">{coverageChips(hit)}</span>}
              {!single && value.length > 1 && (
                <>
                  <button type="button" aria-label={`Move ${symbol} up`} disabled={disabled || index === 0} onClick={() => move(symbol, -1)}>
                    ↑
                  </button>
                  <button type="button" aria-label={`Move ${symbol} down`} disabled={disabled || index === value.length - 1} onClick={() => move(symbol, 1)}>
                    ↓
                  </button>
                </>
              )}
              <button type="button" aria-label={`Remove ${symbol}`} disabled={disabled} onClick={() => remove(symbol)}>
                ×
              </button>
            </span>
          );
        })}
        {!value.length && <em>No instrument selected</em>}
      </div>
      <div className="instrument-search-wrap">
      <div className="instrument-search">
        <span aria-hidden="true">⌕</span>
        <input
          type="text"
          value={query}
          disabled={disabled || full}
          placeholder={
            full
              ? `Maximum of ${requirement.maximum} instruments`
              : single
                ? "Type a ticker or name to replace the instrument"
                : "Type a ticker or name, Enter to add"
          }
          autoComplete="off"
          spellCheck={false}
          aria-label="Search instruments"
          onChange={(event) => {
            const next = event.target.value;
            setQuery(next);
            if (!next.trim()) {
              setHits([]);
              setSearchError("");
              setOpen(false);
            }
          }}
          onFocus={() => hits.length && setOpen(true)}
          onBlur={() => window.setTimeout(() => setOpen(false), 120)}
          onKeyDown={onKeyDown}
        />
        <kbd>{searching ? "…" : requirement.resolutions.join("+").toUpperCase()}</kbd>
      </div>
      {open && hits.length > 0 && (
        <ul className="instrument-results" role="listbox">
          {hits.map((hit, index) => {
            const already = value.includes(hit.symbol);
            return (
              <li
                key={hit.symbol}
                role="option"
                aria-selected={index === cursor}
                className={`${index === cursor ? "active" : ""}${already ? " added" : ""}`}
                onMouseEnter={() => setCursor(index)}
                onMouseDown={(event) => {
                  event.preventDefault();
                  add(hit);
                }}
              >
                <strong>{hit.symbol}</strong>
                <span className="instrument-name">{hit.name || "—"}</span>
                <span className="instrument-meta">
                  {hit.asset_class} · {hit.exchange}
                  {hit.currency ? ` · ${hit.currency}` : ""}
                  {hit.status !== "active" ? ` · ${hit.status}` : ""}
                </span>
                <span className="instrument-cov">{coverageChips(hit)}</span>
                {already && <em>added</em>}
              </li>
            );
          })}
        </ul>
      )}
      {open && query.trim() && !hits.length && !searching && !searchError && (
        <p className="instrument-empty">
          No instruments with {requirement.resolutions.join(" + ")} data match “{query.trim()}”.
        </p>
      )}
      {searchError && <p className="instrument-empty error">{searchError}</p>}
      </div>
      <small className="instrument-note">{requirement.note}</small>
    </div>
  );
}

function StrategyCatalog({
  strategies,
  runs,
  busy,
  onOpen,
  onCode,
}: {
  strategies: Strategy[];
  runs: Run[];
  busy: boolean;
  onOpen: (id: string) => void;
  onCode: (id: string) => void;
}) {
  const [query, setQuery] = useState("");
  const [groupBy, setGroupBy] = useState<CatalogGroupBy>("asset");
  const [statusFilter, setStatusFilter] = useState("all");
  const [sortKey, setSortKey] = useState<CatalogSortKey>("name");
  const [sortDir, setSortDir] = useState<"asc" | "desc">("asc");
  const [collapsed, setCollapsed] = useState<string[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);

  const runStats = useMemo(() => {
    const stats = new Map<string, CatalogRunStats>();
    for (const run of runs) {
      if (!run.strategy_id) continue;
      const current = stats.get(run.strategy_id) ?? { count: 0, complete: 0 };
      current.count += 1;
      if (run.status === "Complete") current.complete += 1;
      if (!current.last || run.created_at > current.last) current.last = run.created_at;
      stats.set(run.strategy_id, current);
    }
    return stats;
  }, [runs]);

  const statuses = useMemo(
    () =>
      Array.from(new Set(strategies.map((strategy) => strategy.status))).sort(
        (a, b) => (CATALOG_STATUS_RANK[a] ?? 9) - (CATALOG_STATUS_RANK[b] ?? 9),
      ),
    [strategies],
  );

  const groups = useMemo<CatalogGroup[]>(() => {
    const needle = query.trim().toLowerCase();
    const visible = strategies.filter((strategy) => {
      if (statusFilter !== "all" && strategy.status !== statusFilter) return false;
      if (!needle) return true;
      return [
        strategy.name,
        strategy.id,
        strategy.version,
        strategy.description,
        strategy.asset_scope,
        strategy.status,
      ]
        .join(" ")
        .toLowerCase()
        .includes(needle);
    });
    if (groupBy === "none") {
      return [
        {
          key: "all",
          label: "All strategies",
          rows: orderCatalogRows(visible, sortKey, sortDir, runStats),
        },
      ];
    }
    const buckets = new Map<string, Strategy[]>();
    for (const strategy of visible) {
      const label = groupBy === "asset" ? strategy.asset_scope : strategy.status;
      buckets.set(label, [...(buckets.get(label) ?? []), strategy]);
    }
    const keys = Array.from(buckets.keys()).sort((a, b) =>
      groupBy === "status"
        ? (CATALOG_STATUS_RANK[a] ?? 9) - (CATALOG_STATUS_RANK[b] ?? 9)
        : a.localeCompare(b),
    );
    return keys.map((key) => ({
      key,
      label: key,
      rows: orderCatalogRows(buckets.get(key) ?? [], sortKey, sortDir, runStats),
    }));
  }, [strategies, query, statusFilter, groupBy, sortKey, sortDir, runStats]);

  const visibleRows = useMemo(
    () => groups.flatMap((group) => (collapsed.includes(group.key) ? [] : group.rows)),
    [groups, collapsed],
  );
  const matchCount = groups.reduce((sum, group) => sum + group.rows.length, 0);
  const selected =
    visibleRows.find((row) => row.strategy.id === selectedId)?.strategy ??
    visibleRows[0]?.strategy ??
    null;
  const selectedStats = selected ? runStats.get(selected.id) : undefined;
  const selectedBase = selected?.base_strategy_id
    ? strategies.find((strategy) => strategy.id === selected.base_strategy_id)
    : undefined;

  function toggleSort(key: CatalogSortKey) {
    if (sortKey === key) setSortDir((dir) => (dir === "asc" ? "desc" : "asc"));
    else {
      setSortKey(key);
      setSortDir(key === "runs" || key === "last" ? "desc" : "asc");
    }
  }
  function toggleGroup(key: string) {
    setCollapsed((current) =>
      current.includes(key) ? current.filter((item) => item !== key) : [...current, key],
    );
  }
  function moveSelection(offset: number) {
    if (!visibleRows.length) return;
    const index = visibleRows.findIndex((row) => row.strategy.id === selected?.id);
    const next = Math.min(visibleRows.length - 1, Math.max(0, index + offset));
    setSelectedId(visibleRows[next].strategy.id);
  }
  function onTableKeyDown(event: KeyboardEvent<HTMLDivElement>) {
    if (event.key === "ArrowDown") {
      event.preventDefault();
      moveSelection(1);
    } else if (event.key === "ArrowUp") {
      event.preventDefault();
      moveSelection(-1);
    } else if (event.key === "Enter" && selected) {
      event.preventDefault();
      onOpen(selected.id);
    }
  }

  const sortMark = (key: CatalogSortKey) =>
    sortKey === key ? (sortDir === "asc" ? " ▲" : " ▼") : "";
  const formatDate = (value?: string) =>
    value ? new Date(value).toLocaleDateString() : "—";
  let rowNumber = 0;

  return (
    <section className="catalog">
      <div className="section-intro">
        <p className="eyebrow">Versioned implementations</p>
        <h2>Strategy catalog</h2>
        <p>
          Frozen strategy identity is separate from each parameter preset and
          immutable run. Select a row to inspect it; double-click or press Enter
          to open.
        </p>
      </div>
      <div className="catalog-toolbar">
        <label className="catalog-search">
          <span aria-hidden="true">⌕</span>
          <input
            type="search"
            placeholder="Filter by name, id, asset, or description"
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            aria-label="Filter strategies"
          />
        </label>
        <div className="catalog-segment" role="group" aria-label="Group strategies by">
          <span>Group</span>
          {(
            [
              ["asset", "Asset"],
              ["status", "Status"],
              ["none", "Flat"],
            ] as [CatalogGroupBy, string][]
          ).map(([value, label]) => (
            <button
              key={value}
              type="button"
              className={groupBy === value ? "active" : ""}
              onClick={() => setGroupBy(value)}
            >
              {label}
            </button>
          ))}
        </div>
        <label className="catalog-select">
          <span>Status</span>
          <select
            value={statusFilter}
            onChange={(event) => setStatusFilter(event.target.value)}
          >
            <option value="all">All</option>
            {statuses.map((status) => (
              <option key={status} value={status}>
                {status}
              </option>
            ))}
          </select>
        </label>
        <span className="catalog-count">
          {matchCount} / {strategies.length} strategies
        </span>
      </div>
      <div className="catalog-layout">
        <section className="panel catalog-monitor">
          <div className="terminal-panel-title">
            <span>STRAT</span> STRATEGY MONITOR
            <em>{groupBy === "none" ? "FLAT" : `BY ${groupBy.toUpperCase()}`}</em>
          </div>
          <div
            className="table-wrap catalog-table"
            tabIndex={0}
            onKeyDown={onTableKeyDown}
            aria-label="Strategy table. Use arrow keys to move and Enter to open."
          >
            <table>
              <thead>
                <tr>
                  <th className="catalog-num">#</th>
                  <th className="catalog-sortable" onClick={() => toggleSort("name")}>
                    Strategy{sortMark("name")}
                  </th>
                  <th className="catalog-sortable" onClick={() => toggleSort("version")}>
                    Ver{sortMark("version")}
                  </th>
                  <th className="catalog-sortable" onClick={() => toggleSort("status")}>
                    Status{sortMark("status")}
                  </th>
                  <th className="catalog-sortable" onClick={() => toggleSort("asset")}>
                    Assets{sortMark("asset")}
                  </th>
                  <th className="catalog-sortable catalog-right" onClick={() => toggleSort("runs")}>
                    Runs{sortMark("runs")}
                  </th>
                  <th className="catalog-sortable catalog-right" onClick={() => toggleSort("last")}>
                    Last run{sortMark("last")}
                  </th>
                  <th className="catalog-right"></th>
                </tr>
              </thead>
              <tbody>
                {groups.map((group) => {
                  const isCollapsed = collapsed.includes(group.key);
                  return (
                    <Fragment key={group.key}>
                      {groupBy !== "none" && (
                        <tr
                          className="catalog-group"
                          onClick={() => toggleGroup(group.key)}
                        >
                          <td colSpan={8}>
                            <span aria-hidden="true">{isCollapsed ? "▸" : "▾"}</span>
                            {group.label}
                            <small>{group.rows.length}</small>
                          </td>
                        </tr>
                      )}
                      {!isCollapsed &&
                        group.rows.map(({ strategy, depth }) => {
                          rowNumber += 1;
                          const stats = runStats.get(strategy.id);
                          const active = selected?.id === strategy.id;
                          return (
                            <tr
                              key={strategy.id}
                              className={
                                active
                                  ? "catalog-row selected"
                                  : strategy.runnable
                                    ? "catalog-row"
                                    : "catalog-row blocked"
                              }
                              onClick={() => setSelectedId(strategy.id)}
                              onDoubleClick={() => onOpen(strategy.id)}
                            >
                              <td className="catalog-num">{rowNumber}</td>
                              <td>
                                <div
                                  className="catalog-name"
                                  style={{ paddingLeft: `${depth * 18}px` }}
                                >
                                  {depth > 0 && <span className="catalog-branch">└</span>}
                                  <strong>{strategy.name}</strong>
                                  {strategy.custom && <i className="catalog-tag">custom</i>}
                                  <small>{strategy.id}</small>
                                </div>
                              </td>
                              <td>
                                <span className="version">{strategy.version}</span>
                              </td>
                              <td>
                                <span className={catalogStatusClass(strategy.status)}>
                                  {strategy.status}
                                </span>
                              </td>
                              <td>{strategy.asset_scope}</td>
                              <td className="catalog-right catalog-numeric">
                                {stats?.count ?? 0}
                              </td>
                              <td className="catalog-right catalog-numeric">
                                {formatDate(stats?.last)}
                              </td>
                              <td className="catalog-right">
                                <button
                                  type="button"
                                  className="text-action"
                                  disabled={busy}
                                  onClick={(event) => {
                                    event.stopPropagation();
                                    onOpen(strategy.id);
                                  }}
                                >
                                  {strategy.runnable ? "Open" : "Review"}
                                </button>
                              </td>
                            </tr>
                          );
                        })}
                    </Fragment>
                  );
                })}
                {!matchCount && (
                  <tr>
                    <td colSpan={8} className="catalog-empty">
                      No strategies match “{query}”.
                    </td>
                  </tr>
                )}
              </tbody>
            </table>
          </div>
        </section>
        <aside className="panel catalog-inspector">
          <div className="terminal-panel-title">
            <span>DES</span> DESCRIPTION
          </div>
          {selected ? (
            <div className="catalog-inspector-body">
              <div className="catalog-inspector-head">
                <span className="strategy-icon">
                  {selected.name.slice(0, 2).toUpperCase()}
                </span>
                <div>
                  <h3>{selected.name}</h3>
                  <p>
                    <span className="version">{selected.version}</span>
                    <span className={catalogStatusClass(selected.status)}>
                      {selected.status}
                    </span>
                  </p>
                </div>
              </div>
              <p className="catalog-description">{selected.description}</p>
              <dl>
                <div>
                  <dt>Id</dt>
                  <dd>{selected.id}</dd>
                </div>
                <div>
                  <dt>Assets</dt>
                  <dd>{selected.asset_scope}</dd>
                </div>
                <div>
                  <dt>Runnable</dt>
                  <dd>{selected.runnable ? "Yes" : `No · ${selected.status}`}</dd>
                </div>
                <div>
                  <dt>Config</dt>
                  <dd>{selected.config_path}</dd>
                </div>
                {selectedBase && (
                  <div>
                    <dt>Based on</dt>
                    <dd>
                      <button
                        type="button"
                        className="catalog-link"
                        onClick={() => setSelectedId(selectedBase.id)}
                      >
                        {selectedBase.name} {selectedBase.version}
                      </button>
                    </dd>
                  </div>
                )}
                {selected.source_sha256 && (
                  <div>
                    <dt>Source</dt>
                    <dd>{selected.source_sha256.slice(0, 16)}…</dd>
                  </div>
                )}
                <div>
                  <dt>Runs</dt>
                  <dd>
                    {selectedStats
                      ? `${selectedStats.count} total · ${selectedStats.complete} complete · last ${formatDate(selectedStats.last)}`
                      : "None in the UI catalog"}
                  </dd>
                </div>
              </dl>
              <div className="catalog-inspector-actions">
                <button
                  type="button"
                  className={selected.runnable ? "primary-action" : "secondary-action"}
                  disabled={busy}
                  onClick={() => onOpen(selected.id)}
                >
                  {selected.runnable ? "Open strategy →" : "Review requirements →"}
                </button>
                <button
                  type="button"
                  className="secondary-action"
                  disabled={busy}
                  onClick={() => onCode(selected.id)}
                >
                  View source code
                </button>
              </div>
            </div>
          ) : (
            <p className="catalog-empty">Nothing selected.</p>
          )}
        </aside>
      </div>
    </section>
  );
}

type RunSortKey =
  | "created"
  | "name"
  | "window"
  | "label"
  | "cagr"
  | "sharpe"
  | "sortino"
  | "drawdown";

function formatPercent(value?: number | null, digits = 1) {
  return value == null || Number.isNaN(value) ? "—" : `${value.toFixed(digits)}%`;
}
function formatRatio(value?: number | null) {
  return value == null || Number.isNaN(value) ? "—" : value.toFixed(2);
}
function drawdownValue(run: Run): number | null {
  const value = run.metrics?.max_drawdown_percent;
  return value == null ? null : -Math.abs(value);
}
function runWindow(run: Run) {
  const start = run.start_date ?? run.metrics?.start;
  const end = run.end_date ?? run.metrics?.end;
  return start ? `${start} → ${end ?? "…"}` : "Preserved result";
}
function metricsLine(run: Run) {
  const metrics = run.metrics;
  if (!metrics) return run.metrics_cached ? "No metrics available" : "Metrics indexing…";
  return `CAGR ${formatPercent(metrics.cagr_percent)} · Sharpe ${formatRatio(metrics.sharpe)} · Sortino ${formatRatio(metrics.sortino)} · DD ${formatPercent(drawdownValue(run))}`;
}
function signClass(value?: number | null) {
  if (value == null || Number.isNaN(value)) return "metric-muted";
  return value >= 0 ? "metric-pos" : "metric-neg";
}

function StarButton({
  run,
  onToggle,
  disabled,
  large,
}: {
  run: Run;
  onToggle: () => void;
  disabled?: boolean;
  large?: boolean;
}) {
  return (
    <button
      type="button"
      className={`star-toggle${run.starred ? " active" : ""}${large ? " large" : ""}`}
      aria-pressed={run.starred}
      aria-label={run.starred ? `Unstar ${run.name}` : `Star ${run.name}`}
      title={
        run.starred
          ? "Starred · available to Portfolios"
          : "Star to keep this run and expose it to Portfolios"
      }
      disabled={disabled}
      onClick={(event) => {
        event.stopPropagation();
        onToggle();
      }}
    >
      {run.starred ? "★" : "☆"}
      {large && <span>{run.starred ? "Starred" : "Star run"}</span>}
    </button>
  );
}

function RunHistoryTable({
  runs,
  onOpen,
  onToggleStar,
  busy,
}: {
  runs: Run[];
  onOpen: (id: string) => void;
  onToggleStar: (run: Run) => void;
  busy: boolean;
}) {
  const [sortKey, setSortKey] = useState<RunSortKey>("created");
  const [sortDir, setSortDir] = useState<"asc" | "desc">("desc");
  const [starredOnly, setStarredOnly] = useState(false);
  const [label, setLabel] = useState("all");
  const labels = useMemo(
    () => Array.from(new Set(runs.map((run) => run.research_label))).sort(),
    [runs],
  );
  const rows = useMemo(() => {
    const filtered = runs.filter(
      (run) =>
        (!starredOnly || run.starred) && (label === "all" || run.research_label === label),
    );
    const value = (run: Run): string | number | null => {
      switch (sortKey) {
        case "created":
          return run.created_at;
        case "name":
          return run.name.toLowerCase();
        case "window":
          return run.start_date ?? run.metrics?.start ?? "";
        case "label":
          return run.research_label;
        case "cagr":
          return run.metrics?.cagr_percent ?? null;
        case "sharpe":
          return run.metrics?.sharpe ?? null;
        case "sortino":
          return run.metrics?.sortino ?? null;
        case "drawdown":
          return drawdownValue(run);
      }
    };
    return [...filtered].sort((a, b) => {
      const left = value(a);
      const right = value(b);
      if (left == null && right == null) return 0;
      if (left == null) return 1;
      if (right == null) return -1;
      const order = left < right ? -1 : left > right ? 1 : 0;
      return sortDir === "asc" ? order : -order;
    });
  }, [runs, sortKey, sortDir, starredOnly, label]);
  const starredCount = runs.filter((run) => run.starred).length;
  const indexing = runs.filter(
    (run) => run.status === "Complete" && !run.metrics_cached,
  ).length;

  function toggleSort(key: RunSortKey) {
    if (sortKey === key) setSortDir((current) => (current === "asc" ? "desc" : "asc"));
    else {
      setSortKey(key);
      setSortDir(key === "name" || key === "label" ? "asc" : "desc");
    }
  }
  const mark = (key: RunSortKey) =>
    sortKey === key ? (sortDir === "asc" ? " ▲" : " ▼") : "";
  const header = (key: RunSortKey, text: string, textColumn = false) => (
    <th
      className={`catalog-sortable${textColumn ? " col-text" : ""}`}
      onClick={() => toggleSort(key)}
    >
      {text}
      {mark(key)}
    </th>
  );

  return (
    <details className="panel runs-table strategy-run-history run-history" open>
      <summary className="strategy-run-history-summary">
        <div>
          <p className="eyebrow">Immutable evidence</p>
          <h2>Historical runs</h2>
        </div>
        <div className="strategy-run-history-meta">
          <span className="market-date">
            {runs.length} {runs.length === 1 ? "RUN" : "RUNS"}
            {starredCount ? ` · ${starredCount} ★` : ""}
          </span>
          <span className="history-toggle">Show / hide</span>
        </div>
      </summary>
      <div className="run-history-toolbar">
        <div className="catalog-segment" role="group" aria-label="Show runs">
          <span>Show</span>
          <button
            type="button"
            className={starredOnly ? "" : "active"}
            onClick={() => setStarredOnly(false)}
          >
            All
          </button>
          <button
            type="button"
            className={starredOnly ? "active" : ""}
            onClick={() => setStarredOnly(true)}
          >
            ★ Starred {starredCount}
          </button>
        </div>
        <label className="catalog-select">
          <span>Label</span>
          <select value={label} onChange={(event) => setLabel(event.target.value)}>
            <option value="all">All</option>
            {labels.map((item) => (
              <option key={item} value={item}>
                {item}
              </option>
            ))}
          </select>
        </label>
        {indexing > 0 && (
          <span className="catalog-count">{indexing} indexing metrics…</span>
        )}
        <span className="catalog-count">
          {rows.length} / {runs.length} runs
        </span>
      </div>
      {rows.length ? (
        <div className="table-wrap">
          <table>
            <thead>
              <tr>
                <th className="star-col" aria-label="Starred">
                  ★
                </th>
                {header("name", "Run", true)}
                {header("window", "Window", true)}
                {header("label", "Label", true)}
                {header("cagr", "CAGR %")}
                {header("sharpe", "Sharpe")}
                {header("sortino", "Sortino")}
                {header("drawdown", "Max DD %")}
                {header("created", "Created", true)}
                <th></th>
              </tr>
            </thead>
            <tbody>
              {rows.map((run) => (
                <tr
                  key={run.id}
                  className={run.starred ? "starred" : ""}
                  onClick={() => onOpen(run.id)}
                >
                  <td className="star-col">
                    <StarButton
                      run={run}
                      disabled={busy}
                      onToggle={() => onToggleStar(run)}
                    />
                  </td>
                  <td className="col-text run-name">
                    <strong>{run.name}</strong>
                    <small>{run.id}</small>
                  </td>
                  <td className="col-text">{runWindow(run)}</td>
                  <td className="col-text">{run.research_label}</td>
                  <td className={`catalog-numeric ${signClass(run.metrics?.cagr_percent)}`}>
                    {formatNumber(run.metrics?.cagr_percent)}
                  </td>
                  <td className="catalog-numeric">{formatRatio(run.metrics?.sharpe)}</td>
                  <td className="catalog-numeric">{formatRatio(run.metrics?.sortino)}</td>
                  <td className="catalog-numeric metric-neg">
                    {formatNumber(drawdownValue(run))}
                  </td>
                  <td className="col-text" title={new Date(run.created_at).toLocaleString()}>
                    {new Date(run.created_at).toLocaleDateString()}
                  </td>
                  <td>
                    <button
                      className="text-action"
                      type="button"
                      onClick={(event) => {
                        event.stopPropagation();
                        onOpen(run.id);
                      }}
                    >
                      Open →
                    </button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      ) : (
        <div className="empty-state strategy-run-empty">
          {runs.length
            ? "No runs match the current filter."
            : "No backtests have been saved for this strategy yet. Its first run will appear here automatically."}
        </div>
      )}
    </details>
  );
}

/** Run form for one-file SDK strategies: platform settings plus manifest-declared parameters. */
function SdkForm({
  manifest,
  detail,
  parameters,
  setParam,
  advanced,
  busy,
}: {
  manifest: SdkManifest;
  detail: StrategyDetail;
  parameters: Overrides;
  setParam: (key: keyof Overrides, value: number | boolean | string | string[]) => void;
  advanced: boolean;
  busy: boolean;
}) {
  const resolution = (parameters.resolution as string | undefined) ?? "daily";
  const session = (parameters.session as string | undefined) ?? "regular";
  const requirement: InstrumentRequirement = {
    parameter: "symbols",
    mode: "multiple",
    resolutions: [resolution],
    suffixes: [],
    asset_classes: [],
    maximum: 50,
    note: `One strategy instance per symbol on a shared account. Every symbol needs ${resolution} bars.`,
  };
  const symbols =
    parameters.symbols ??
    (Array.isArray(detail.default_parameters.symbols)
      ? (detail.default_parameters.symbols as string[])
      : []);
  const universe = symbols.find((symbol) => symbol.startsWith("universe:")) ?? "";
  const explicitSymbols = symbols.filter((symbol) => !symbol.startsWith("universe:"));
  const maxEntries =
    (parameters.max_entries_per_day as number | undefined) ??
    (typeof detail.default_parameters.max_entries_per_day === "number"
      ? (detail.default_parameters.max_entries_per_day as number)
      : 0);
  const maxOpen = (parameters.max_open_positions as number | undefined) ?? 0;
  const tieBreak =
    (parameters.tie_break as string | undefined) ??
    (typeof detail.default_parameters.tie_break === "string"
      ? (detail.default_parameters.tie_break as string)
      : "priority");
  const seed =
    (parameters.random_seed as number | undefined) ??
    (typeof detail.default_parameters.random_seed === "number"
      ? (detail.default_parameters.random_seed as number)
      : 0);
  const visible = manifest.params.filter((param) => advanced || param.tier !== "advanced");
  const hidden = manifest.params.length - visible.length;
  const value = (param: SdkParam) => {
    const current = parameters[param.name];
    return current === undefined ? param.default : current;
  };
  return (
    <>
      <div className="form-section">
        <h3>Data and sizing</h3>
        <div className="field-grid five">
          <label>
            Universe
            <select
              value={universe}
              onChange={(event) =>
                setParam(
                  "symbols",
                  event.target.value
                    ? [event.target.value]
                    : explicitSymbols.length
                      ? explicitSymbols
                      : ["SPY.US"],
                )
              }
            >
              <option value="">Selected symbols</option>
              <option value="universe:stocks">All US common stocks</option>
              <option value="universe:etfs">All US ETFs</option>
              <option value="universe:all">Stocks and ETFs</option>
            </select>
            <small>
              {manifest.screen_universe
                ? "daily screen picks the intraday candidates"
                : "explicit lists load every symbol up front; no cap. Intraday bars across a whole universe exceed memory for long windows, and the engine refuses such runs up front"}
            </small>
          </label>
          {!universe && (
            <div className="instrument-field">
              <span className="instrument-label">
                Symbols<small>one instance per symbol, shared account · list order is priority</small>
              </span>
              <InstrumentPicker
                requirement={requirement}
                value={explicitSymbols}
                onChange={(next) => setParam("symbols", next)}
                disabled={busy}
              />
            </div>
          )}
          <label>
            Bar resolution
            <select
              value={resolution}
              onChange={(event) => setParam("resolution", event.target.value)}
            >
              <option value="daily">Daily</option>
              <option value="5m">5 minute</option>
              <option value="1m">1 minute</option>
            </select>
            <small>the strategy is resolution-agnostic</small>
          </label>
          <label>
            Session
            <select
              value={session}
              onChange={(event) => setParam("session", event.target.value)}
              disabled={resolution === "daily"}
            >
              <option value="regular">Regular hours</option>
              <option value="extended">Extended hours</option>
            </select>
            <small>09:30–16:00 ET or all bars</small>
          </label>
          <label>
            Position size %
            <input
              type="number"
              min="0"
              max="1000"
              step="any"
              value={((parameters.position_percent as number | undefined) ?? 1) * 100}
              onChange={(event) => setParam("position_percent", +event.target.value / 100)}
            />
            <small>of equity per Size::Default entry</small>
          </label>
          <label>
            Minimum price
            <input
              type="number"
              min="0"
              step="any"
              value={(parameters.min_price as number | undefined) ?? 1}
              onChange={(event) => setParam("min_price", +event.target.value)}
            />
            <small>skip entries below this price · 0 disables</small>
          </label>
          <label>
            Initial capital
            <input
              type="number"
              min="0"
              step="any"
              value={(parameters.initial_capital as number | undefined) ?? 100000}
              onChange={(event) => setParam("initial_capital", +event.target.value)}
            />
          </label>
          <label>
            Warm-up
            <input type="text" value={`${manifest.warmup_bars} bars`} readOnly />
            <small>declared by the strategy</small>
          </label>
        </div>
      </div>
      <div className="form-section">
        <h3>Entry limits</h3>
        <div className="field-grid five">
          <label>
            Max entries per day
            <input
              type="number"
              min="0"
              step="1"
              value={maxEntries}
              onChange={(event) => setParam("max_entries_per_day", Math.max(0, Math.round(+event.target.value)))}
            />
            <small>0 = unlimited · applied at fill time across all symbols</small>
          </label>
          <label>
            Max open positions
            <input
              type="number"
              min="0"
              step="1"
              value={maxOpen}
              onChange={(event) => setParam("max_open_positions", Math.max(0, Math.round(+event.target.value)))}
            />
            <small>0 = unlimited</small>
          </label>
          <label>
            Max gross exposure ×
            <input
              type="number"
              min="0.1"
              step="any"
              value={(parameters.max_gross_exposure as number | undefined) ?? Math.max(1, ((parameters.position_percent as number | undefined) ?? 1) * Math.max(1, maxOpen))}
              onChange={(event) => setParam("max_gross_exposure", +event.target.value)}
            />
            <small>buying power as a multiple of equity · fills beyond it are cut or rejected</small>
          </label>
          <label>
            Tie-break
            <select value={tieBreak} onChange={(event) => setParam("tie_break", event.target.value)}>
              <option value="priority">Priority (strategy order)</option>
              <option value="random">Random (seeded)</option>
              <option value="alphabetical">Alphabetical</option>
            </select>
            <small>when more signals than slots on one bar</small>
          </label>
          <label>
            Random seed
            <input
              type="number"
              min="0"
              step="1"
              value={seed}
              disabled={tieBreak !== "random"}
              onChange={(event) => setParam("random_seed", Math.max(0, Math.round(+event.target.value)))}
            />
            <small>reproducible shuffles</small>
          </label>
        </div>
      </div>
      <div className="form-section">
        <h3>
          Strategy parameters
          {hidden > 0 && <small className="sdk-hidden-note"> · {hidden} advanced hidden</small>}
        </h3>
        {visible.length ? (
          <div className="field-grid five">
            {visible.map((param) => {
              const current = value(param);
              const unit = param.unit ? ` (${param.unit})` : "";
              if (param.kind === "bool") {
                return (
                  <label key={param.name} className="toggle-label">
                    <input
                      type="checkbox"
                      checked={Boolean(current)}
                      onChange={(event) => setParam(param.name, event.target.checked)}
                    />
                    <span>
                      {param.label}
                      {param.help ? ` · ${param.help}` : ""}
                    </span>
                  </label>
                );
              }
              if (param.kind === "choice") {
                return (
                  <label key={param.name}>
                    {param.label}
                    <select
                      value={String(current)}
                      onChange={(event) => setParam(param.name, event.target.value)}
                    >
                      {(param.choices ?? []).map((choice) => (
                        <option key={choice} value={choice}>
                          {choice}
                        </option>
                      ))}
                    </select>
                    {param.help && <small>{param.help}</small>}
                  </label>
                );
              }
              const step = param.step ?? (param.kind === "int" ? 1 : 0.1);
              return (
                <label key={param.name}>
                  {param.label}
                  {unit}
                  <input
                    type="number"
                    min={param.min ?? undefined}
                    max={param.max ?? undefined}
                    step={step}
                    value={Number(current)}
                    onChange={(event) =>
                      setParam(
                        param.name,
                        param.kind === "int"
                          ? Math.round(+event.target.value)
                          : +event.target.value,
                      )
                    }
                  />
                  <small>
                    {param.help || (param.min != null || param.max != null
                      ? `${param.min ?? "…"} to ${param.max ?? "…"}`
                      : "")}
                  </small>
                </label>
              );
            })}
          </div>
        ) : (
          <p className="empty-state">This strategy declares no parameters.</p>
        )}
      </div>
    </>
  );
}

function StrategyWorkspace({
  detail,
  parameters,
  costsEnabled,
  costProfiles,
  selectedCostProfileId,
  advanced,
  today,
  busy,
  connected,
  presetName,
  setPresetName,
  setAdvanced,
  setSelectedCostProfileId,
  setParam,
  applyPreset,
  savePreset,
  submitRun,
  viewCode,
  openRun,
  toggleStar,
  back,
}: {
  detail: StrategyDetail;
  parameters: Overrides;
  costsEnabled: boolean;
  costProfiles: CostProfile[];
  selectedCostProfileId: string;
  advanced: boolean;
  today: string;
  busy: boolean;
  connected: boolean;
  presetName: string;
  setPresetName: (value: string) => void;
  setAdvanced: (value: boolean) => void;
  setSelectedCostProfileId: (value: string) => void;
  setParam: (
    key: keyof Overrides,
    value: number | boolean | string | string[],
  ) => void;
  applyPreset: (preset?: Preset) => void;
  savePreset: () => void;
  submitRun: (event: FormEvent<HTMLFormElement>) => void;
  viewCode: () => void;
  openRun: (id: string) => void;
  toggleStar: (run: Run) => void;
  back: () => void;
}) {
  const sdk = detail.sdk ?? null;
  const title = detail.strategy.name;
  const summary = sdk?.description ?? detail.strategy.description;
  const badges = sdk
    ? [
        sdk.asset_scope,
        `${sdk.default_resolution ?? "any"} bars`,
        sdk.allows_short ? "long/short" : "long/cash",
        sdk.screen_universe ? "screened universe" : `${sdk.params.length} parameters`,
      ]
    : [detail.strategy.asset_scope, detail.strategy.status];
  return (
    <>
      <section className="strategy-hero">
        <div>
          <button className="back-action" onClick={back}>
            ← Strategy catalog
          </button>
          <p className="eyebrow">
            {detail.strategy.status} · {detail.strategy.version}
          </p>
          <h2>{title}</h2>
          <p>{summary}</p>
        </div>
        <div className="strategy-hero-actions">
          {sdk ? (
          <div className="strategy-badges">
            <span>{sdk.asset_scope}</span>
            <span>{(parameters.resolution as string | undefined) ?? "any bars"}</span>
            <span>{sdk.allows_short ? "long/short" : "long/cash"}</span>
            {sdk.screen_universe && <span>screened universe</span>}
            {sdk.daily_context && !sdk.screen_universe && <span>daily context</span>}
            {sdk.default_max_entries_per_day ? (
              <span>{sdk.default_max_entries_per_day}/day cap</span>
            ) : null}
          </div>
        ) : (
          <div className="strategy-badges">
            {badges.map((badge) => (
              <span key={badge}>{badge}</span>
            ))}
          </div>
        )}
          <button className="secondary-action" type="button" onClick={viewCode}>
            View source code →
          </button>
        </div>
      </section>
      <div className="strategy-layout">
        <section className="panel rules-panel">
          <div className="panel-head">
            <div>
              <p className="eyebrow">Human-readable definition</p>
              <h2>Production rules</h2>
            </div>
          </div>
          <ol>
            {detail.rules.map((rule, index) => (
              <li key={rule}>
                <span>{String(index + 1).padStart(2, "0")}</span>
                <p>{rule}</p>
              </li>
            ))}
          </ol>
          <div className="assumption-strip">
            <div>
              <span>Entry</span>
              <strong>Per strategy rules</strong>
            </div>
            <div>
              <span>Exit</span>
              <strong>Per strategy rules</strong>
            </div>
            <div>
              <span>Default costs</span>
              <strong>{costProfiles.find((profile) => profile.id === selectedCostProfileId)?.name ?? "Strategy default"}</strong>
            </div>
          </div>
        </section>
        <section className="panel preset-panel">
          <div className="panel-head">
            <div>
              <p className="eyebrow">Reusable configurations</p>
              <h2>Presets</h2>
            </div>
          </div>
          <div className="preset-save">
            <input
              value={presetName}
              maxLength={80}
              placeholder="Name current settings"
              onChange={(event) => setPresetName(event.target.value)}
            />
            <button className="text-action" type="button" onClick={savePreset}>
              Save +
            </button>
          </div>
          <button className="preset-row active" onClick={() => applyPreset()}>
            <span>
              <strong>Production defaults</strong>
              <small>Frozen strategy config</small>
            </span>
            <em>BASE</em>
          </button>
          {detail.presets.map((preset) => (
            <button
              className="preset-row"
              key={preset.id}
              onClick={() => applyPreset(preset)}
            >
              <span>
                <strong>{preset.name}</strong>
                <small>{new Date(preset.created_at).toLocaleString()}</small>
              </span>
              <em>{preset.costs_enabled ? "NET" : "GROSS"}</em>
            </button>
          ))}
        </section>
      </div>
      <section className="panel config-panel">
        <div className="panel-head">
          <div>
            <p className="eyebrow">Controlled research</p>
            <h2>Configure immutable run</h2>
          </div>
          <div className="mode-switch">
            <button
              className={!advanced ? "active" : ""}
              type="button"
              onClick={() => setAdvanced(false)}
            >
              Simple
            </button>
            <button
              className={advanced ? "active" : ""}
              type="button"
              onClick={() => setAdvanced(true)}
            >
              Advanced
            </button>
          </div>
        </div>
        <form onSubmit={submitRun} noValidate>
          <div className="form-section">
            <h3>Research identity</h3>
            <div className="field-grid">
              <label>
                Research label
                <select name="research_label" defaultValue="Development">
                  <option>Development</option>
                  <option>Validation</option>
                  <option>Final holdout</option>
                  <option>Post-selection</option>
                  <option>Research</option>
                </select>
              </label>
              <label>
                Start
                <input
                  name="start_date"
                  type="date"
                  defaultValue="2020-01-01"
                  required
                />
              </label>
              <label>
                End
                <input
                  name="end_date"
                  type="date"
                  defaultValue={today}
                  required
                />
              </label>
              <label>
                Run name <span>optional</span>
                <input
                  name="name"
                  placeholder={`${detail.strategy.name} · development baseline`}
                />
              </label>
              <label>
                Cost profile
                <select
                  value={selectedCostProfileId}
                  disabled={!costsEnabled}
                  onChange={(event) => setSelectedCostProfileId(event.target.value)}
                >
                  {costProfiles
                    .filter((profile) => sdk || profile.asset_class === detail.strategy.asset_scope || profile.asset_class === "Any" ||
                      (detail.strategy.asset_scope.includes("US") && profile.asset_class === "US equities"))
                    .map((profile) => <option value={profile.id} key={profile.id}>{profile.name}</option>)}
                </select>
              </label>
            </div>
          </div>
          {sdk && (
            <SdkForm
              manifest={sdk}
              detail={detail}
              parameters={parameters}
              setParam={setParam}
              advanced={advanced}
              busy={busy}
            />
          )}
          <div className="run-submit">
            <div>
              <strong>Write-once output</strong>
              <p>
                The exact parameters, costs, dates, logs, trades, coverage,
                equity, and report are preserved together.
              </p>
            </div>
            <button
              className="primary-action"
              type="submit"
              disabled={busy || !connected}
            >
              {busy ? "Queueing…" : "Queue backtest →"}
            </button>
          </div>
        </form>
      </section>
      <RunHistoryTable
        runs={detail.runs}
        onOpen={openRun}
        onToggleStar={toggleStar}
        busy={busy}
      />
    </>
  );
}

export default function Home() {
  const [terminalMode, setTerminalMode] = useState(true);
  useEffect(() => {
    let stored: string | null = null;
    try {
      stored = window.localStorage.getItem("bt-display-mode");
    } catch {
      // storage unavailable; keep the terminal default
    }
    if (stored !== "modern") return;
    const timer = window.setTimeout(() => setTerminalMode(false), 0);
    return () => window.clearTimeout(timer);
  }, []);
  function chooseDisplayMode(terminal: boolean) {
    setTerminalMode(terminal);
    try {
      window.localStorage.setItem("bt-display-mode", terminal ? "terminal" : "modern");
    } catch {
      // ignore storage failures
    }
  }
  const [view, setView] = useState<View>("dashboard");
  const [stripStatus, setStripStatus] = useState<{ latest_spy_date?: string } | null>(null);
  useEffect(() => {
    let cancelled = false;
    const load = () =>
      fetch(`${API}/data/status`, { cache: "no-store" })
        .then((response) => (response.ok ? response.json() : null))
        .then((body) => {
          if (!cancelled) setStripStatus(body as { latest_spy_date?: string } | null);
        })
        .catch(() => undefined);
    void load();
    const timer = window.setInterval(() => void load(), 5 * 60 * 1000);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, []);
  const [dashboard, setDashboard] = useState<Dashboard>(emptyDashboard);
  const [runs, setRuns] = useState<Run[]>([]);
  const [strategyDetail, setStrategyDetail] = useState<StrategyDetail | null>(
    null,
  );
  const [runDetail, setRunDetail] = useState<RunDetail | null>(null);
  const [coverageDetail, setCoverageDetail] = useState<RunDetail | null>(null);
  const [dataStatus, setDataStatus] = useState<DataStatus | null>(null);
  const [sweeps, setSweeps] = useState<SweepRecord[]>([]);
  const [sweepDetail, setSweepDetail] = useState<SweepDetail | null>(null);
  const [compareDetails, setCompareDetails] = useState<RunDetail[]>([]);
  const [portfolios, setPortfolios] = useState<PortfolioRecord[]>([]);
  const [portfolioDetail, setPortfolioDetail] = useState<RunDetail | null>(null);
  const [costProfiles, setCostProfiles] = useState<CostProfile[]>([]);
  const [selectedCostProfileId, setSelectedCostProfileId] = useState("us-equities-default");
  const [automations, setAutomations] = useState<AutomationSchedule[]>([]);
  const [codeSource, setCodeSource] = useState<StrategySource | null>(null);
  const [strategyDrafts, setStrategyDrafts] = useState<StrategyDraft[]>([]);
  const [strategyDraftDetail, setStrategyDraftDetail] =
    useState<StrategyDraftDetail | null>(null);
  const [parameters, setParameters] = useState<Overrides>({});
  const [costOverridesDirty, setCostOverridesDirty] = useState(false);
  const [costsEnabled, setCostsEnabled] = useState(true);
  const [advanced, setAdvanced] = useState(false);
  const [presetName, setPresetName] = useState("");
  const [connected, setConnected] = useState(false);
  const [busy, setBusy] = useState(false);
  const [notice, setNotice] = useState("");
  const [error, setError] = useState("");

  const refresh = useCallback(async () => {
    try {
      const [dashboardResponse, costResponse, automationResponse] = await Promise.all([
        fetch(`${API}/dashboard`, { cache: "no-store" }),
        fetch(`${API}/cost-profiles`, { cache: "no-store" }),
        fetch(`${API}/automations`, { cache: "no-store" }),
      ]);
      if (!dashboardResponse.ok) throw new Error();
      setDashboard(await dashboardResponse.json());
      if (costResponse.ok) setCostProfiles(await costResponse.json());
      if (automationResponse.ok) setAutomations(await automationResponse.json());
      setConnected(true);
    } catch {
      setConnected(false);
    }
  }, []);
  useEffect(() => {
    const initial = window.setTimeout(refresh, 0);
    const timer = window.setInterval(refresh, 3000);
    return () => {
      window.clearTimeout(initial);
      window.clearInterval(timer);
    };
  }, [refresh]);

  const openStrategy = useCallback(async (id = "iwm_mdy_gap_fade_v1") => {
    setBusy(true);
    setError("");
    try {
      const response = await fetch(
        `${API}/strategies/${encodeURIComponent(id)}`,
        { cache: "no-store" },
      );
      const body = await response.json();
      if (!response.ok) throw new Error(body.error);
      setStrategyDetail(body);
      setParameters(body.default_parameters);
      setCostOverridesDirty(false);
      setCostsEnabled(true);
      setSelectedCostProfileId("us-equities-default");
      setView("strategy");
    } catch (caught) {
      setError(
        caught instanceof Error ? caught.message : "Could not load strategy",
      );
    } finally {
      setBusy(false);
    }
  }, []);
  const selectCodeStrategy = useCallback(async (id: string) => {
    setBusy(true);
    setError("");
    try {
      const response = await fetch(
        `${API}/strategies/${encodeURIComponent(id)}/source`,
        { cache: "no-store" },
      );
      const body = await response.json();
      if (!response.ok) throw new Error(body.error);
      setCodeSource(body);
      setStrategyDraftDetail(null);
    } catch (caught) {
      setError(
        caught instanceof Error ? caught.message : "Could not load strategy source",
      );
    } finally {
      setBusy(false);
    }
  }, []);
  const selectStrategyDraft = useCallback(async (id: string) => {
    setBusy(true);
    setError("");
    try {
      const response = await fetch(
        `${API}/strategy-drafts/${encodeURIComponent(id)}`,
        { cache: "no-store" },
      );
      const body = await response.json();
      if (!response.ok) throw new Error(body.error);
      setStrategyDraftDetail(body);
      setCodeSource(null);
    } catch (caught) {
      setError(
        caught instanceof Error ? caught.message : "Could not load strategy draft",
      );
    } finally {
      setBusy(false);
    }
  }, []);
  const openCode = useCallback(
    async (strategyId?: string) => {
      setBusy(true);
      setError("");
      try {
        const [draftResponse, sourceResponse] = await Promise.all([
          fetch(`${API}/strategy-drafts`, { cache: "no-store" }),
          fetch(
            `${API}/strategies/${encodeURIComponent(strategyId ?? "iwm_mdy_gap_fade_v1")}/source`,
            { cache: "no-store" },
          ),
        ]);
        const draftsBody = await draftResponse.json();
        const sourceBody = await sourceResponse.json();
        if (!draftResponse.ok) throw new Error(draftsBody.error);
        if (!sourceResponse.ok) throw new Error(sourceBody.error);
        setStrategyDrafts(draftsBody);
        setCodeSource(sourceBody);
        setStrategyDraftDetail(null);
        setView("code");
      } catch (caught) {
        setError(
          caught instanceof Error ? caught.message : "Could not open code workspace",
        );
      } finally {
        setBusy(false);
      }
    },
    [],
  );
  const toggleRunStar = useCallback(async (run: Run) => {
    try {
      const response = await fetch(
        `${API}/runs/${encodeURIComponent(run.id)}/star`,
        {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({ starred: !run.starred }),
        },
      );
      const body = (await response.json()) as Run & { error?: string };
      if (!response.ok) throw new Error(body.error ?? "Could not update star");
      const patch = (item: Run) =>
        item.id === body.id ? { ...item, starred: body.starred } : item;
      setRuns((current) => current.map(patch));
      setStrategyDetail((current) =>
        current ? { ...current, runs: current.runs.map(patch) } : current,
      );
      setRunDetail((current) =>
        current && current.run.id === body.id
          ? { ...current, run: { ...current.run, starred: body.starred } }
          : current,
      );
      setDashboard((current) => ({
        ...current,
        recent_runs: current.recent_runs.map(patch),
      }));
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : "Could not update star");
    }
  }, []);
  const openRuns = useCallback(async () => {
    setBusy(true);
    try {
      const response = await fetch(`${API}/runs`, { cache: "no-store" });
      setRuns(await response.json());
      setView("runs");
    } finally {
      setBusy(false);
    }
  }, []);
  useEffect(() => {
    if (view !== "strategies" || runs.length) return;
    let cancelled = false;
    fetch(`${API}/runs`, { cache: "no-store" })
      .then((response) => (response.ok ? response.json() : []))
      .then((body) => {
        if (!cancelled && Array.isArray(body)) setRuns(body as Run[]);
      })
      .catch(() => undefined);
    return () => {
      cancelled = true;
    };
  }, [view, runs.length]);
  const loadCoverageRun = useCallback(async (id: string) => {
    if (!id) return;
    setBusy(true);
    setError("");
    try {
      const response = await fetch(`${API}/runs/${encodeURIComponent(id)}`, {
        cache: "no-store",
      });
      const body = await response.json();
      if (!response.ok) throw new Error(body.error);
      setCoverageDetail(body);
    } catch (caught) {
      setError(
        caught instanceof Error ? caught.message : "Could not load coverage",
      );
    } finally {
      setBusy(false);
    }
  }, []);
  const openData = useCallback(async () => {
    setBusy(true);
    setError("");
    try {
      const response = await fetch(`${API}/runs`, { cache: "no-store" });
      const body: Run[] = await response.json();
      setRuns(body);
      const preferred =
        body.find((run) => run.id === "run-20260830T011827.314481Z") ??
        body.find((run) => !run.legacy && run.status === "Complete");
      setView("data");
      if (preferred) {
        const detailResponse = await fetch(
          `${API}/runs/${encodeURIComponent(preferred.id)}`,
          { cache: "no-store" },
        );
        const detailBody = await detailResponse.json();
        if (detailResponse.ok) setCoverageDetail(detailBody);
      }
    } finally {
      setBusy(false);
    }
  }, []);
  const refreshDataStatus = useCallback(async () => {
    const statusResponse = await fetch(`${API}/data/status`, { cache: "no-store" });
    if (statusResponse.ok) setDataStatus(await statusResponse.json());
  }, []);
  useEffect(() => {
    if (view !== "data") return;
    const initial = window.setTimeout(() => void refreshDataStatus(), 0);
    const timer = window.setInterval(() => void refreshDataStatus(), 30_000);
    return () => {
      window.clearTimeout(initial);
      window.clearInterval(timer);
    };
  }, [view, refreshDataStatus]);
  const openRun = useCallback(async (id: string) => {
    setBusy(true);
    setError("");
    try {
      const response = await fetch(`${API}/runs/${encodeURIComponent(id)}`, {
        cache: "no-store",
      });
      const body = await response.json();
      if (!response.ok) throw new Error(body.error);
      setRunDetail(body);
      setView("run");
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : "Could not load run");
    } finally {
      setBusy(false);
    }
  }, []);

  const selectSweep = useCallback(async (id: string) => {
    const response = await fetch(`${API}/sweeps/${encodeURIComponent(id)}`, {
      cache: "no-store",
    });
    const body = await response.json();
    if (!response.ok) throw new Error(body.error);
    setSweepDetail(body);
  }, []);

  const openCompare = useCallback(async () => {
    setBusy(true);
    setError("");
    try {
      const [sweepResponse, runResponse] = await Promise.all([
        fetch(`${API}/sweeps`, { cache: "no-store" }),
        fetch(`${API}/runs`, { cache: "no-store" }),
      ]);
      const sweepBody: SweepRecord[] = await sweepResponse.json();
      const runBody: Run[] = await runResponse.json();
      if (!sweepResponse.ok || !runResponse.ok)
        throw new Error("Could not load comparison workspace");
      setSweeps(sweepBody);
      setRuns(runBody);
      setView("compare");
      if (sweepBody.length) await selectSweep(sweepBody[0].id);
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : "Could not load comparisons");
    } finally {
      setBusy(false);
    }
  }, [selectSweep]);

  const selectPortfolio = useCallback(async (runId: string) => {
    setBusy(true);
    setError("");
    try {
      const response = await fetch(`${API}/runs/${encodeURIComponent(runId)}`, { cache: "no-store" });
      const body = await response.json();
      if (!response.ok) throw new Error(body.error);
      setPortfolioDetail(body);
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : "Could not load portfolio");
    } finally {
      setBusy(false);
    }
  }, []);

  const openPortfolios = useCallback(async () => {
    setBusy(true);
    setError("");
    try {
      const [portfolioResponse, runResponse] = await Promise.all([
        fetch(`${API}/portfolios`, { cache: "no-store" }),
        fetch(`${API}/runs`, { cache: "no-store" }),
      ]);
      const portfolioBody: PortfolioRecord[] = await portfolioResponse.json();
      const runBody: Run[] = await runResponse.json();
      if (!portfolioResponse.ok || !runResponse.ok)
        throw new Error("Could not load Portfolio Builder");
      setPortfolios(portfolioBody);
      setRuns(runBody);
      setView("portfolios");
      if (portfolioBody.length) await selectPortfolio(portfolioBody[0].run_id);
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : "Could not load Portfolio Builder");
    } finally {
      setBusy(false);
    }
  }, [selectPortfolio]);

  const openCosts = useCallback(async () => {
    setBusy(true);
    setError("");
    try {
      const response = await fetch(`${API}/cost-profiles`, { cache: "no-store" });
      const body = await response.json();
      if (!response.ok) throw new Error(body.error);
      setCostProfiles(body);
      setView("costs");
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : "Could not load cost profiles");
    } finally {
      setBusy(false);
    }
  }, []);

  const activeSweepId = sweepDetail?.sweep.id;
  useEffect(() => {
    if (view !== "compare" || !activeSweepId) return;
    const timer = window.setInterval(async () => {
      try {
        const [sweepResponse, listResponse] = await Promise.all([
          fetch(`${API}/sweeps/${encodeURIComponent(activeSweepId)}`, { cache: "no-store" }),
          fetch(`${API}/sweeps`, { cache: "no-store" }),
        ]);
        if (sweepResponse.ok) setSweepDetail(await sweepResponse.json());
        if (listResponse.ok) setSweeps(await listResponse.json());
      } catch {
        // Keep the last complete research snapshot visible during a transient refresh.
      }
    }, 4000);
    return () => window.clearInterval(timer);
  }, [view, activeSweepId]);

  const activeDraftId = strategyDraftDetail?.draft.id;
  const activeValidationStatus = strategyDraftDetail?.validation?.status;
  useEffect(() => {
    if (
      view !== "code" ||
      !activeDraftId ||
      !activeValidationStatus ||
      !["queued", "running"].includes(activeValidationStatus)
    )
      return;
    const draftId = activeDraftId;
    const timer = window.setInterval(async () => {
      try {
        const response = await fetch(
          `${API}/strategy-drafts/${encodeURIComponent(draftId)}`,
          { cache: "no-store" },
        );
        if (!response.ok) return;
        const body: StrategyDraftDetail = await response.json();
        setStrategyDraftDetail(body);
        setStrategyDrafts((current) =>
          current.map((draft) =>
            draft.id === body.draft.id ? body.draft : draft,
          ),
        );
      } catch {
        // Preserve the last editor state during a transient validation refresh.
      }
    }, 2000);
    return () => window.clearInterval(timer);
  }, [view, activeDraftId, activeValidationStatus]);

  function navigate(target: View | "new") {
    setNotice("");
    setError("");
    if (target === "new" || target === "strategy") void openStrategy();
    else if (target === "strategies") setView("strategies");
    else if (target === "runs") void openRuns();
    else if (target === "compare") void openCompare();
    else if (target === "portfolios") void openPortfolios();
    else if (target === "data") void openData();
    else if (target === "costs") void openCosts();
    else if (target === "code") void openCode();
    else setView(target);
  }

  async function createStrategyDraft(request: Record<string, unknown>) {
    setBusy(true);
    setError("");
    setNotice("");
    try {
      const response = await fetch(`${API}/strategy-drafts`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(request),
      });
      const body = await response.json();
      if (!response.ok) throw new Error(body.error);
      setStrategyDraftDetail(body);
      setCodeSource(null);
      setStrategyDrafts((current) => [
        body.draft,
        ...current.filter((draft) => draft.id !== body.draft.id),
      ]);
      setNotice(`Draft “${body.draft.name}” created from ${body.draft.base_strategy_id}.`);
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : "Draft could not be created");
    } finally {
      setBusy(false);
    }
  }

  async function saveStrategyDraftFile(
    draftId: string,
    path: string,
    content: string,
  ) {
    setBusy(true);
    setError("");
    try {
      const response = await fetch(
        `${API}/strategy-drafts/${encodeURIComponent(draftId)}/files`,
        {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({ path, content }),
        },
      );
      const body = await response.json();
      if (!response.ok) throw new Error(body.error);
      setStrategyDraftDetail(body);
      setStrategyDrafts((current) =>
        current.map((draft) =>
          draft.id === body.draft.id ? body.draft : draft,
        ),
      );
      setNotice(`${path} saved; prior validation was invalidated.`);
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : "Source file could not be saved");
    } finally {
      setBusy(false);
    }
  }

  async function validateStrategyDraft(draftId: string, action: string) {
    setBusy(true);
    setError("");
    setNotice("");
    try {
      const response = await fetch(
        `${API}/strategy-drafts/${encodeURIComponent(draftId)}/validate`,
        {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({ action }),
        },
      );
      const body = await response.json();
      if (!response.ok) throw new Error(body.error);
      setStrategyDraftDetail((current) =>
        current
          ? {
              ...current,
              draft: { ...current.draft, status: "validating", last_validation_id: body.id },
              validation: body,
            }
          : current,
      );
      setNotice(`${action} queued in the isolated strategy worker.`);
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : "Validation could not start");
    } finally {
      setBusy(false);
    }
  }

  async function buildStrategyDraft(draftId: string) {
    setBusy(true);
    setError("");
    setNotice("Building dev engine… this takes a minute or two on first build.");
    try {
      const response = await fetch(
        `${API}/strategy-drafts/${encodeURIComponent(draftId)}/build`,
        { method: "POST" },
      );
      const body = (await response.json()) as {
        error?: string;
        strategies?: Strategy[];
      };
      if (!response.ok) throw new Error(body.error ?? "Dev build failed");
      const built = body.strategies ?? [];
      await refresh();
      setNotice(
        built.length
          ? `Dev engine built. ${built.map((item) => item.name).join(", ")} ready to run.`
          : "Dev engine built, but it exposes no SDK strategies.",
      );
      if (built[0]) await openStrategy(built[0].id);
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : "Dev build failed");
    } finally {
      setBusy(false);
    }
  }
  async function releaseStrategyDraft(
    draftId: string,
    request: Record<string, unknown>,
  ) {
    setBusy(true);
    setError("");
    setNotice("");
    try {
      const response = await fetch(
        `${API}/strategy-drafts/${encodeURIComponent(draftId)}/release`,
        {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify(request),
        },
      );
      const body = await response.json();
      if (!response.ok) throw new Error(body.error);
      setCodeSource(body);
      setStrategyDraftDetail(null);
      const draftsResponse = await fetch(`${API}/strategy-drafts`, { cache: "no-store" });
      if (draftsResponse.ok) setStrategyDrafts(await draftsResponse.json());
      await refresh();
      setNotice(`Released ${body.strategy.name} ${body.strategy.version} as an immutable runnable strategy.`);
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : "Strategy release failed");
    } finally {
      setBusy(false);
    }
  }

  async function createSweep(request: Record<string, unknown>) {
    setBusy(true);
    setError("");
    setNotice("");
    try {
      const response = await fetch(`${API}/sweeps`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(request),
      });
      const body = await response.json();
      if (!response.ok) throw new Error(body.error);
      setSweepDetail(body);
      setSweeps((current) => [body.sweep, ...current.filter((item) => item.id !== body.sweep.id)]);
      setNotice(`${body.sweep.id} queued ${body.sweep.configuration_count} development configurations.`);
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : "Sweep could not be queued");
    } finally {
      setBusy(false);
    }
  }

  async function buildPortfolio(request: Record<string, unknown>) {
    setBusy(true);
    setError("");
    setNotice("");
    try {
      const response = await fetch(`${API}/portfolios`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(request),
      });
      const body = await response.json();
      if (!response.ok) throw new Error(body.error);
      setPortfolios((current) => [body, ...current.filter((item) => item.id !== body.id)]);
      await selectPortfolio(body.run_id);
      setNotice(`${body.id} saved with ${body.component_count} immutable source sleeves.`);
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : "Portfolio could not be built");
    } finally {
      setBusy(false);
    }
  }

  async function createCostProfile(request: Record<string, unknown>) {
    setBusy(true);
    setError("");
    try {
      const response = await fetch(`${API}/cost-profiles`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(request),
      });
      const body = await response.json();
      if (!response.ok) throw new Error(body.error);
      setCostProfiles((current) => [body, ...current]);
      setNotice(`Cost profile “${body.name}” saved as an immutable version.`);
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : "Cost profile could not be saved");
    } finally {
      setBusy(false);
    }
  }

  async function automationAction(id: string, action: "toggle" | "run") {
    setBusy(true);
    setError("");
    try {
      const response = await fetch(`${API}/automations/${encodeURIComponent(id)}/${action}`, { method: "POST" });
      const body = await response.json();
      if (!response.ok) throw new Error(body.error);
      setAutomations((current) => current.map((item) => item.id === body.id ? body : item));
      setNotice(action === "run" ? `${body.name} started.` : `${body.name} is now ${body.enabled ? "active" : "paused"}.`);
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : "Automation action failed");
    } finally {
      setBusy(false);
    }
  }

  async function createAutomation(request: Record<string, unknown>) {
    setBusy(true);
    setError("");
    try {
      const response = await fetch(`${API}/automations`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(request),
      });
      const body = await response.json();
      if (!response.ok) throw new Error(body.error);
      setAutomations((current) => [...current, body]);
      setNotice(`Automation “${body.name}” saved.`);
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : "Automation could not be saved");
    } finally {
      setBusy(false);
    }
  }

  async function toggleCompareRun(id: string) {
    if (compareDetails.some((item) => item.run.id === id)) {
      setCompareDetails((current) => current.filter((item) => item.run.id !== id));
      return;
    }
    if (compareDetails.length >= 4) return;
    setBusy(true);
    try {
      const response = await fetch(`${API}/runs/${encodeURIComponent(id)}`, { cache: "no-store" });
      const body = await response.json();
      if (!response.ok) throw new Error(body.error);
      setCompareDetails((current) => [...current, body]);
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : "Run could not be compared");
    } finally {
      setBusy(false);
    }
  }
  function setParam(
    key: keyof Overrides,
    value: number | boolean | string | string[],
  ) {
    if (
      key === "slippage_ticks" ||
      key === "commission_per_share" ||
      key === "all_in_round_trip_bps" ||
      key === "entry_slippage_ticks" ||
      key === "exit_slippage_ticks" ||
      key === "commission_per_share_per_fill"
    ) {
      setCostOverridesDirty(true);
    }
    setParameters((current) => ({ ...current, [key]: value }));
  }
  function applyPreset(preset?: Preset) {
    if (!strategyDetail) return;
    setParameters({
      ...(strategyDetail.default_parameters as Overrides),
      ...(preset?.parameters ?? {}),
    });
    setCostsEnabled(preset?.costs_enabled ?? true);
    setCostOverridesDirty(
      Boolean(
        preset &&
          ("slippage_ticks" in preset.parameters ||
            "commission_per_share" in preset.parameters ||
            "all_in_round_trip_bps" in preset.parameters ||
            "entry_slippage_ticks" in preset.parameters ||
            "exit_slippage_ticks" in preset.parameters ||
            "commission_per_share_per_fill" in preset.parameters),
      ),
    );
  }
  async function savePreset() {
    const name = presetName.trim();
    if (!name) {
      setError("Enter a preset name first.");
      return;
    }
    setBusy(true);
    setError("");
    try {
      const response = await fetch(`${API}/presets`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          strategy_id: strategyDetail?.strategy.id,
          name,
          parameters,
          costs_enabled: costsEnabled,
        }),
      });
      const body = await response.json();
      if (!response.ok) throw new Error(body.error);
      setStrategyDetail((current) =>
        current ? { ...current, presets: [body, ...current.presets] } : current,
      );
      setPresetName("");
      setNotice(`Preset “${name}” saved.`);
    } catch (caught) {
      setError(
        caught instanceof Error ? caught.message : "Preset could not be saved",
      );
    } finally {
      setBusy(false);
    }
  }
  async function submitRun(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const form = new FormData(event.currentTarget);
    setBusy(true);
    setError("");
    setNotice("");
    try {
      const submittedParameters = { ...parameters };
      if (strategyDetail?.instruments) {
        const selected = selectedInstrumentSymbols(strategyDetail, parameters);
        if (!selected?.length) {
          throw new Error("Select at least one instrument before queueing the run.");
        }
      }
      if (!costOverridesDirty || !costsEnabled) {
        delete submittedParameters.slippage_ticks;
        delete submittedParameters.commission_per_share;
        delete submittedParameters.all_in_round_trip_bps;
        delete submittedParameters.entry_slippage_ticks;
        delete submittedParameters.exit_slippage_ticks;
        delete submittedParameters.commission_per_share_per_fill;
      }
      const response = await fetch(`${API}/jobs`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          strategy_id: strategyDetail?.strategy.id,
          start_date: form.get("start_date"),
          end_date: form.get("end_date"),
          research_label: form.get("research_label"),
          name: form.get("name"),
          parameters: submittedParameters,
          costs_enabled: costsEnabled,
          cost_profile_id: selectedCostProfileId,
        }),
      });
      const body = await response.json();
      if (!response.ok) throw new Error(body.error);
      setNotice(`${body.id} queued with a frozen parameter snapshot.`);
      setView("dashboard");
      await refresh();
    } catch (caught) {
      setError(
        caught instanceof Error ? caught.message : "Run could not be queued",
      );
    } finally {
      setBusy(false);
    }
  }

  async function startDataUpdate() {
    setBusy(true);
    setError("");
    setNotice("");
    try {
      const response = await fetch(`${API}/data/update-eod`, {
        method: "POST",
      });
      const body = await response.json();
      if (!response.ok) throw new Error(body.error);
      setNotice(
        `${body.id} started. The data library will refresh in place.`,
      );
      await refreshDataStatus();
    } catch (caught) {
      setError(
        caught instanceof Error
          ? caught.message
          : "Data update could not start",
      );
    } finally {
      setBusy(false);
    }
  }


  const today = new Date().toISOString().slice(0, 10);
  const activeNav =
    view === "strategy"
      ? "Strategies"
      : view === "run"
        ? "Runs"
        : view[0].toUpperCase() + view.slice(1);
  const runningJob = dashboard.jobs.find((job) =>
    ["running", "queued"].includes(job.status),
  );

  return (
    <main
      className="app-shell"
      data-theme={terminalMode ? "terminal" : "modern"}
    >
      <aside className="sidebar">
        <div className="brand">
          <span className="brand-mark">BT</span>
          <div>
            <strong>Tessera</strong>
            <small>Research Console</small>
          </div>
        </div>
        <nav aria-label="Primary navigation">
          {navItems.map(([label, symbol, target]) => (
            <button
              className={activeNav === label ? "nav-item active" : "nav-item"}
              key={label}
              type="button"
              onClick={() => navigate(target)}
            >
              <span aria-hidden="true">{symbol}</span>
              {label}
            </button>
          ))}
        </nav>
        <div className="sidebar-foot">
          <span
            className={connected ? "engine-light" : "engine-light offline"}
          />
          <div>
            <strong>{connected ? "Engine ready" : "API offline"}</strong>
            <small>Local Mac · {dashboard.active_jobs} active</small>
          </div>
        </div>
      </aside>
      <section className="workspace">
        <div className="terminal-ribbon" aria-hidden="true">
          <strong>BT &lt;GO&gt;</strong>
          <span>LOCAL RESEARCH CONSOLE</span>
          <span>IMMUTABLE EVIDENCE</span>
          <span>{connected ? "ENGINE ONLINE" : "ENGINE OFFLINE"}</span>
          <Clock />
        </div>
        <header className="topbar">
          <div>
            <p className="eyebrow">Local research · immutable evidence</p>
            <h1>
              {view === "strategy"
                ? strategyDetail?.strategy.name
                : view === "run"
                  ? runDetail?.run.name
                  : activeNav}
            </h1>
          </div>
          <div className="top-actions">
            <button
              className="theme-toggle"
              type="button"
              onClick={() => chooseDisplayMode(!terminalMode)}
            >
              {terminalMode ? "Modern mode" : "Terminal mode"}
            </button>
            <button
              className="primary-action"
              type="button"
              onClick={() => void openStrategy()}
            >
              ＋ New backtest
            </button>
          </div>
        </header>
        <div className="content">
          {busy && <div className="progress-bar" />}
          {(notice || error) && (
            <div className={error ? "notice error" : "notice"}>
              {error || notice}
            </div>
          )}

          {view === "dashboard" && (
            <>
              <section className="function-bar">
                <span className="fn-tag">HOME</span>
                <strong>Research console</strong>
                <span className="fn-context">
                  {connected ? "ENGINE READY" : "START API"} ·{" "}
                  {dashboard.strategies.length} STRATEGIES ·{" "}
                  {dashboard.historical_reports} HISTORICAL REPORTS ·{" "}
                  {dashboard.active_jobs}/{dashboard.worker_capacity} WORKERS BUSY
                </span>
              </section>
              <section className="metrics">
                <Metric
                  label="Production strategies"
                  value={String(dashboard.production_strategies)}
                  note="Frozen entry points"
                />
                <Metric
                  label="Historical reports"
                  value={String(dashboard.historical_reports)}
                  note="Immutable legacy catalog"
                />
                <Metric
                  label="Running jobs"
                  value={`${dashboard.active_jobs} / ${dashboard.worker_capacity}`}
                  note="Worker capacity"
                />
                <Metric
                  label="Runnable in UI"
                  value={`${dashboard.strategies.filter((item) => item.runnable).length} / ${dashboard.strategies.length}`}
                  note="Stocks, ETFs, FX, and crypto"
                />
              </section>
              <div className="dashboard-grid">
                <section className="panel production-panel">
                  <div className="panel-head">
                    <div>
                      <p className="eyebrow">Production desk</p>
                      <h2>Strategies</h2>
                    </div>
                    <button
                      className="text-action"
                      onClick={() => setView("strategies")}
                    >
                      View catalog →
                    </button>
                  </div>
                  <div className="strategy-list">
                    {dashboard.strategies.map((strategy) => (
                      <article className="strategy-row" key={strategy.id}>
                        <span className="strategy-icon">
                          {strategy.name.slice(0, 2).toUpperCase()}
                        </span>
                        <div className="strategy-main">
                          <div>
                            <strong>{strategy.name}</strong>
                            <span className="version">{strategy.version}</span>
                          </div>
                          <p>{strategy.description}</p>
                        </div>
                        <span className="status production">
                          {strategy.status}
                        </span>
                        <span className="freshness">
                          {strategy.runnable ? "Runnable" : strategy.status}
                        </span>
                        <button
                          onClick={() =>
                            void openStrategy(strategy.id)
                          }
                        >
                          ›
                        </button>
                      </article>
                    ))}
                  </div>
                </section>
                <section className="panel queue-panel">
                  <div className="panel-head">
                    <div>
                      <p className="eyebrow">Worker queue</p>
                      <h2>{runningJob?.status ?? "Quiet—for now"}</h2>
                    </div>
                    <span className="status ready">
                      {runningJob ? "Active" : "Ready"}
                    </span>
                  </div>
                  <div className="queue-visual">
                    <span>01</span>
                    <i />
                    <span>02</span>
                    <i />
                    <span>03</span>
                  </div>
                  {runningJob ? (
                    <>
                      <p>
                        {runningJob.strategy_id} · {runningJob.start_date} → {runningJob.end_date}
                      </p>
                      <JobProgressBar job={runningJob} />
                    </>
                  ) : (
                    <p>New jobs run in isolated processes and survive browser closure.</p>
                  )}
                  <button
                    className="secondary-action"
                    onClick={() => void openStrategy()}
                  >
                    Configure a run
                  </button>
                </section>
                <section className="panel activity-panel">
                  <div className="panel-head">
                    <div>
                      <p className="eyebrow">Recent evidence</p>
                      <h2>Immutable runs</h2>
                    </div>
                    <button
                      className="text-action"
                      onClick={() => void openRuns()}
                    >
                      View all →
                    </button>
                  </div>
                  <table>
                    <thead>
                      <tr>
                        <th>Run</th>
                        <th>Label</th>
                        <th>State</th>
                      </tr>
                    </thead>
                    <tbody>
                      {dashboard.recent_runs.map((run) => (
                        <tr key={run.id} onClick={() => void openRun(run.id)}>
                          <td>{run.name}</td>
                          <td>{run.research_label}</td>
                          <td>
                            <span
                              className={run.legacy ? "legacy-dot" : "live-dot"}
                            />
                            {run.status}
                          </td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </section>
              </div>
            </>
          )}

          {view === "strategies" && (
            <StrategyCatalog
              strategies={dashboard.strategies}
              runs={runs}
              busy={busy}
              onOpen={(id) => void openStrategy(id)}
              onCode={(id) => void openCode(id)}
            />
          )}

          {view === "strategy" && strategyDetail && (
            <StrategyWorkspace
              detail={strategyDetail}
              parameters={parameters}
              costsEnabled={costsEnabled}
              costProfiles={costProfiles}
              selectedCostProfileId={selectedCostProfileId}
              advanced={advanced}
              today={today}
              busy={busy}
              connected={connected}
              presetName={presetName}
              setPresetName={setPresetName}
              setAdvanced={setAdvanced}
              setSelectedCostProfileId={setSelectedCostProfileId}
              setParam={setParam}
              applyPreset={applyPreset}
              savePreset={() => void savePreset()}
              submitRun={submitRun}
              viewCode={() => void openCode(strategyDetail.strategy.id)}
              openRun={(id) => void openRun(id)}
              toggleStar={(run) => void toggleRunStar(run)}
              back={() => setView("strategies")}
            />
          )}

          {view === "code" && (
            <CodeWorkspace
              key={
                strategyDraftDetail
                  ? `${strategyDraftDetail.draft.id}:${strategyDraftDetail.draft.source_sha256}`
                  : `${codeSource?.strategy.id}:${codeSource?.source_sha256}`
              }
              strategies={dashboard.strategies}
              drafts={strategyDrafts}
              source={codeSource}
              draftDetail={strategyDraftDetail}
              busy={busy}
              onSelectStrategy={(id) => void selectCodeStrategy(id)}
              onSelectDraft={(id) => void selectStrategyDraft(id)}
              onCreateDraft={(request) => void createStrategyDraft(request)}
              onSaveFile={(draftId, path, content) =>
                void saveStrategyDraftFile(draftId, path, content)
              }
              onValidate={(draftId, action) =>
                void validateStrategyDraft(draftId, action)
              }
              onBuild={(draftId) => void buildStrategyDraft(draftId)}
              onRelease={(draftId, request) =>
                void releaseStrategyDraft(draftId, request)
              }
            />
          )}

          {view === "compare" && (
            <CompareWorkspace
              strategies={dashboard.strategies}
              runs={runs}
              sweeps={sweeps}
              detail={sweepDetail}
              compareDetails={compareDetails}
              busy={busy}
              onCreate={(request) => void createSweep(request)}
              onSelectSweep={(id) => void selectSweep(id)}
              onToggleRun={(id) => void toggleCompareRun(id)}
              onOpenRun={(id) => void openRun(id)}
            />
          )}

          {view === "portfolios" && (
            <PortfolioWorkspace
              runs={runs}
              portfolios={portfolios}
              detail={portfolioDetail}
              seedRunIds={compareDetails.map((item) => item.run.id)}
              busy={busy}
              onCreate={(request) => void buildPortfolio(request)}
              onSelect={(runId) => void selectPortfolio(runId)}
            />
          )}


          {view === "data" && (
            <>
              <section className="panel data-library-panel">
                <div className="terminal-panel-title">
                  <span>LIB</span> DATA LIBRARY
                  <button
                    type="button"
                    className="text-action source-filter"
                    disabled={busy || dataStatus?.update_job?.status === "running"}
                    onClick={() => void startDataUpdate()}
                  >
                    Run update command
                  </button>
                </div>
                <div className="metrics data-library-metrics">
                  <Metric
                    label="Latest market date"
                    value={dataStatus?.latest_market_date ?? "—"}
                    note={`calendar ${dataStatus?.latest_spy_date ?? "—"}`}
                  />
                  <Metric
                    label="Daily files"
                    value={String(dataStatus?.symbols_on_latest_date ?? "—")}
                    note={`${dataStatus?.universe_symbols ?? "—"} in universe`}
                  />
                  <Metric
                    label="Updated"
                    value={dataStatus?.updated_at_utc ?? "unknown"}
                    note="from the freshness file when configured"
                  />
                  <Metric
                    label="Last update job"
                    value={dataStatus?.update_job?.status ?? "none"}
                    note={dataStatus?.update_job?.error ?? dataStatus?.update_job?.id ?? "local.toml update_command"}
                  />
                </div>
              </section>
              <DataCoverage
                runs={runs}
                detail={coverageDetail}
                onSelect={(id) => void loadCoverageRun(id)}
              />
              <AutomationsWorkspace
                schedules={automations}
                busy={busy}
                onToggle={(id) => void automationAction(id, "toggle")}
                onRun={(id) => void automationAction(id, "run")}
                onCreate={(request) => void createAutomation(request)}
              />
            </>
          )}

          {view === "costs" && (
            <CostsWorkspace
              profiles={costProfiles}
              busy={busy}
              onCreate={(request) => void createCostProfile(request)}
            />
          )}

          {view === "runs" && (
            <>
              <div className="section-intro">
                <p className="eyebrow">Immutable evidence</p>
                <h2>Runs</h2>
                <p>
                  Production UI runs and preserved legacy reports, ordered by
                  creation time.
                </p>
              </div>
              <section className="panel runs-table">
                <div className="table-wrap">
                  <table>
                    <thead>
                      <tr>
                        <th className="star-col" aria-label="Starred">
                          ★
                        </th>
                        <th className="col-text">Run</th>
                        <th className="col-text">Strategy</th>
                        <th className="col-text">Window</th>
                        <th className="col-text">Label</th>
                        <th>CAGR %</th>
                        <th>Sharpe</th>
                        <th>Sortino</th>
                        <th>Max DD %</th>
                        <th className="col-text">State</th>
                        <th className="col-text">Created</th>
                      </tr>
                    </thead>
                    <tbody>
                      {runs.map((run) => (
                        <tr
                          key={run.id}
                          className={run.starred ? "starred" : ""}
                          onClick={() => void openRun(run.id)}
                        >
                          <td className="star-col">
                            <StarButton
                              run={run}
                              disabled={busy}
                              onToggle={() => void toggleRunStar(run)}
                            />
                          </td>
                          <td className="col-text run-name">
                            <strong>{run.name}</strong>
                            <small>{run.id}</small>
                          </td>
                          <td className="col-text">{run.strategy_id ?? "Legacy"}</td>
                          <td className="col-text">{runWindow(run)}</td>
                          <td className="col-text">{run.research_label}</td>
                          <td className={`catalog-numeric ${signClass(run.metrics?.cagr_percent)}`}>
                            {formatNumber(run.metrics?.cagr_percent)}
                          </td>
                          <td className="catalog-numeric">{formatRatio(run.metrics?.sharpe)}</td>
                          <td className="catalog-numeric">{formatRatio(run.metrics?.sortino)}</td>
                          <td className="catalog-numeric metric-neg">
                            {formatNumber(drawdownValue(run))}
                          </td>
                          <td className="col-text">
                            <span
                              className={run.legacy ? "legacy-dot" : "live-dot"}
                            />
                            {run.status}
                          </td>
                          <td className="col-text">
                            {new Date(run.created_at).toLocaleDateString()}
                          </td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </div>
              </section>
            </>
          )}

          {view === "run" && runDetail && (
            <>
              <section className="run-title">
                <div>
                  <button
                    className="back-action"
                    onClick={() => void openRuns()}
                  >
                    ← All runs
                  </button>
                  <p className="eyebrow">
                    {runDetail.run.research_label} · {runDetail.run.status}
                  </p>
                  <h2>{runDetail.run.name}</h2>
                  <p>
                    {runDetail.report
                      ? `${runDetail.report.start} through ${runDetail.report.end} · ${runDetail.report.symbols.join(", ")}`
                      : runDetail.run.artifact_dir}
                  </p>
                </div>
                <div className="top-actions">
                  <StarButton
                    run={runDetail.run}
                    disabled={busy}
                    large
                    onToggle={() => void toggleRunStar(runDetail.run)}
                  />
                  {runDetail.report_url && (
                    <a
                      className="secondary-action link-button"
                      href={`${ORIGIN}${runDetail.report_url}`}
                      target="_blank"
                      rel="noreferrer"
                    >
                      Original HTML ↗
                    </a>
                  )}
                  <button
                    className="primary-action"
                    onClick={() =>
                      void openStrategy(
                        runDetail.run.strategy_id ?? "iwm_mdy_gap_fade_v1",
                      )
                    }
                  >
                    Rerun configuration →
                  </button>
                </div>
              </section>
              <RunReport detail={runDetail} />
              <section className="panel audit-panel">
                <div className="panel-head">
                  <div>
                    <p className="eyebrow">Reproducibility</p>
                    <h2>Frozen configuration</h2>
                  </div>
                  <span className="status production">Immutable</span>
                </div>
                <details>
                  <summary>Show TOML snapshot</summary>
                  <pre>
                    {runDetail.config_text ??
                      "No configuration snapshot is registered for this legacy report."}
                  </pre>
                </details>
                {runDetail.manifest && (
                  <details>
                    <summary>Show run manifest</summary>
                    <pre>{JSON.stringify(runDetail.manifest, null, 2)}</pre>
                  </details>
                )}
              </section>
            </>
          )}
        </div>
        <footer className="status-strip" aria-label="Session status">
          <span className="strip-brand">BT</span>
          <span>{connected ? "Engine online" : "Engine offline"}</span>
          <span>
            Queue {dashboard.active_jobs}/{dashboard.worker_capacity}
            {runningJob?.progress ? ` · ${runningJob.progress.stage} ${runningJob.progress.percent.toFixed(0)}%` : ""}
          </span>
          <span>{dashboard.historical_reports} reports</span>
          {stripStatus?.latest_spy_date && (
            <span>US EOD {stripStatus.latest_spy_date}</span>
          )}
          <span>{terminalMode ? "Terminal" : "Modern"} display</span>
          <Clock />
        </footer>
      </section>
    </main>
  );
}
