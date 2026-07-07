//! `sim`: the 24h simulation runner (architecture §8.3, Phase 3).
//!
//! Runs every shipped traffic scenario with `Policy::default()` — a managed
//! run (real executor, real store, MockNode) against an unmanaged baseline —
//! and writes the demo artifacts:
//!
//! - `ops/sim/report.json` — the raw [`SimReport`]s;
//! - `ops/sim/report.html` — a self-contained (inline CSS + SVG, no external
//!   assets, no scripts) with/without trajectory chart per scenario.
//!
//! Deterministic by construction: the simulated clock, scripted balances and
//! mock fees never touch the wall clock, so two invocations produce
//! byte-identical files. Run with `cargo run -p evenkeel-server --bin sim`
//! (`DATABASE_URL` must point at a migrated Even Keel database).

use std::error::Error;
use std::fmt::Write as _;
use std::path::Path;

use evenkeel_core::{Policy, Shannons, SHANNONS_PER_CKB};
use evenkeel_server::sim::{self, SimReport, SimRun};
use evenkeel_store::Store;

/// Fixed categorical palette for up to three channels, validated (dataviz
/// six-checks) against the dashboard's dark surface `#121211`: worst adjacent
/// CVD ΔE 15.7, all ≥ 3:1 contrast. Assigned by scenario channel order,
/// never cycled; managed/baseline runs share the channel's hue and differ by
/// line style (solid vs dashed) so identity is never color-alone.
const SERIES: [&str; 3] = ["#3987e5", "#199e70", "#c98500"];

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let _ = tracing_subscriber::fmt().with_env_filter("info").try_init();

    let database_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://evenkeel:evenkeel@127.0.0.1:5433/evenkeel".to_string());
    let store = Store::connect(&database_url).await?;
    let policy = Policy::default();

    let mut reports = Vec::new();
    for scenario in sim::all_scenarios() {
        // Unique per scenario and invocation so DB rows and intent ids never
        // collide across runs (the sim clock restarts from the same epoch);
        // nothing derived from this appears in the report files.
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let run_id = format!("{}{:x}{:x}", scenario.name, std::process::id(), nanos);
        let report =
            sim::compare_scenario(&policy, &scenario, &store, &run_id, sim::TICKS_PER_DAY)
                .await?;
        println!(
            "{:<12} managed: fee {} CKB, {} settled / {} failed / {} rejected, \
             mean imbalance {} → {} bp (baseline ends {} bp)",
            report.scenario,
            format_ckb(report.managed.total_fee),
            report.managed.actions_settled,
            report.managed.actions_failed,
            report.managed.actions_rejected,
            report.managed.imbalance_start_mean_bp,
            report.managed.imbalance_end_mean_bp,
            report.baseline.imbalance_end_mean_bp,
        );
        reports.push(report);
    }

    let out_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ops/sim");
    std::fs::create_dir_all(&out_dir)?;
    let mut json = serde_json::to_string_pretty(&reports)?;
    json.push('\n');
    std::fs::write(out_dir.join("report.json"), json)?;
    std::fs::write(out_dir.join("report.html"), render_html(&reports))?;
    println!("wrote {}", out_dir.join("report.{json,html}").display());
    Ok(())
}

/// Shannons → exact CKB decimal string (integer math; display only).
fn format_ckb(shannons: Shannons) -> String {
    let whole = shannons / SHANNONS_PER_CKB;
    let frac = shannons % SHANNONS_PER_CKB;
    if frac == 0 {
        return whole.to_string();
    }
    let digits = format!("{frac:08}");
    format!("{whole}.{}", digits.trim_end_matches('0'))
}

/// Display name for a channel: the id minus the `0xsim_` prefix.
fn short(channel_id: &str) -> &str {
    channel_id.strip_prefix("0xsim_").unwrap_or(channel_id)
}

// ---- chart geometry (floats below are SVG coordinates: display-only) -------

const W: f64 = 960.0;
const H: f64 = 340.0;
const X0: f64 = 52.0;
const X1: f64 = 820.0;
const Y0: f64 = 16.0;
const Y1: f64 = 296.0;

fn x_at(tick: usize, ticks: u32) -> f64 {
    X0 + (X1 - X0) * tick as f64 / (ticks.max(2) - 1) as f64
}

fn y_at(bp: u16) -> f64 {
    Y1 - (Y1 - Y0) * f64::from(bp) / 10_000.0
}

/// SVG path for one trajectory, breaking the line at `None` points.
fn path_d(points: &[Option<u16>], ticks: u32) -> String {
    let mut d = String::new();
    let mut pen_down = false;
    for (i, p) in points.iter().enumerate() {
        match p {
            Some(bp) => {
                let cmd = if pen_down { 'L' } else { 'M' };
                let _ = write!(d, "{cmd}{:.1} {:.1}", x_at(i, ticks), y_at(*bp));
                pen_down = true;
            }
            None => pen_down = false,
        }
    }
    d
}

fn horizontal_guide(out: &mut String, bp: u16, label: &str, dashed: bool) {
    let y = y_at(bp);
    let dash = if dashed { r#" stroke-dasharray="3 5""# } else { "" };
    let _ = writeln!(
        out,
        r##"<line x1="{X0}" y1="{y:.1}" x2="{X1}" y2="{y:.1}" stroke="#32322f" stroke-width="1"{dash}/>
<text x="{:.1}" y="{:.1}" text-anchor="end" class="axis">{label}</text>"##,
        X0 - 8.0,
        y + 4.0,
    );
}

/// One scenario's SVG: threshold guides, baseline (dashed) and managed
/// (solid) usable-ratio trajectories per channel, direct end labels.
fn render_chart(r: &SimReport) -> String {
    let mut s = String::new();
    let _ = writeln!(
        s,
        r#"<svg viewBox="0 0 {W} {H}" role="img" aria-label="Usable ratio over 24 simulated hours, {}: managed vs unmanaged">"#,
        r.scenario
    );

    // Recessive grid + labeled guides: axis band, thresholds, target.
    horizontal_guide(&mut s, 10_000, "100%", false);
    horizontal_guide(&mut s, r.saturated_above_bp, "80% saturated", true);
    horizontal_guide(&mut s, r.target_ratio_bp, "50% target", true);
    horizontal_guide(&mut s, r.depleted_below_bp, "20% depleted", true);
    horizontal_guide(&mut s, 0, "0%", false);

    // X axis: hours.
    for h in [0u32, 6, 12, 18, 24] {
        let tick = (h * 12).min(r.ticks - 1) as usize;
        let x = x_at(tick, r.ticks);
        let _ = writeln!(
            s,
            r#"<text x="{x:.1}" y="{:.1}" text-anchor="middle" class="axis">{h}h</text>"#,
            Y1 + 20.0
        );
    }

    // Baseline first (dashed, recessive), managed on top (solid).
    for (run, dashed) in [(&r.baseline, true), (&r.managed, false)] {
        for (ci, id) in r.channels.iter().enumerate() {
            let Some(points) = run.trajectories.get(id) else { continue };
            let color = SERIES[ci % SERIES.len()];
            let extra = if dashed { r#" stroke-dasharray="5 4" opacity="0.45""# } else { "" };
            let _ = writeln!(
                s,
                r#"<path d="{}" fill="none" stroke="{color}" stroke-width="2" stroke-linejoin="round"{extra}/>"#,
                path_d(points, r.ticks)
            );
        }
    }

    // Direct end labels on the managed lines, nudged apart vertically.
    let mut labels: Vec<(f64, String, &str)> = r
        .channels
        .iter()
        .enumerate()
        .filter_map(|(ci, id)| {
            let bp = r.managed.trajectories.get(id)?.iter().rev().flatten().next()?;
            Some((y_at(*bp), short(id).to_string(), SERIES[ci % SERIES.len()]))
        })
        .collect();
    labels.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut last_y = f64::MIN;
    for (y, name, color) in &mut labels {
        *y = y.max(last_y + 13.0).clamp(Y0 + 4.0, Y1);
        last_y = *y;
        let _ = writeln!(
            s,
            r#"<text x="{:.1}" y="{:.1}" class="dlabel"><tspan fill="{color}">●</tspan> {name}</text>"#,
            X1 + 8.0,
            *y + 4.0,
        );
    }

    s.push_str("</svg>\n");
    s
}

fn stat(out: &mut String, label: &str, value: &str) {
    let _ = writeln!(
        out,
        r#"<div class="stat"><div class="stat-label">{label}</div><div class="stat-value mono">{value}</div></div>"#
    );
}

fn run_summary(run: &SimRun) -> String {
    format!(
        "{} settled / {} failed / {} rejected",
        run.actions_settled, run.actions_failed, run.actions_rejected
    )
}

/// The whole self-contained report page (inline CSS + SVG, no scripts, no
/// external assets), on the dashboard's dark palette.
fn render_html(reports: &[SimReport]) -> String {
    let mut body = String::new();
    for r in reports {
        let reduction = r.baseline.imbalance_end.saturating_sub(r.managed.imbalance_end);
        let reduction_pct = match (reduction * 1_000).checked_div(r.baseline.imbalance_end) {
            Some(permille) => format!("{}.{}%", permille / 10, permille % 10),
            None => "0.0%".to_string(),
        };

        let mut legend = String::new();
        for (ci, id) in r.channels.iter().enumerate() {
            let _ = write!(
                legend,
                r#"<span class="key"><span class="chip" style="background:{}"></span>{}</span>"#,
                SERIES[ci % SERIES.len()],
                short(id)
            );
        }

        let mut stats = String::new();
        stat(&mut stats, "Fees spent (managed)", &format!("{} CKB", format_ckb(r.managed.total_fee)));
        stat(&mut stats, "Daily fee cap", &format!("{} CKB", format_ckb(r.daily_fee_cap)));
        stat(&mut stats, "Actions (managed)", &run_summary(&r.managed));
        stat(
            &mut stats,
            "Mean imbalance, managed",
            &format!("{} → {} bp", r.managed.imbalance_start_mean_bp, r.managed.imbalance_end_mean_bp),
        );
        stat(
            &mut stats,
            "Mean imbalance, unmanaged",
            &format!("{} → {} bp", r.baseline.imbalance_start_mean_bp, r.baseline.imbalance_end_mean_bp),
        );
        stat(&mut stats, "Net imbalance reduction vs unmanaged", &reduction_pct);

        let _ = write!(
            body,
            r##"<section class="card">
<h2>{name}</h2>
<p class="desc">{desc}</p>
<div class="legend">{legend}<span class="key"><svg width="26" height="8"><line x1="0" y1="4" x2="26" y2="4" stroke="#c3c2b7" stroke-width="2"/></svg>managed</span><span class="key"><svg width="26" height="8"><line x1="0" y1="4" x2="26" y2="4" stroke="#c3c2b7" stroke-width="2" stroke-dasharray="5 4" opacity="0.45"/></svg>unmanaged</span></div>
<div class="chart">{chart}</div>
<div class="stats">{stats}</div>
</section>
"##,
            name = r.scenario,
            desc = r.description,
            chart = render_chart(r),
        );
    }

    format!(
        r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Even Keel — 24h simulation report</title>
<style>
:root {{
  --surface-0:#121211; --surface-1:#1a1a19; --surface-2:#232322;
  --border:#32322f; --text-primary:#ffffff; --text-secondary:#c3c2b7;
  --text-muted:#8b8a80;
  --mono:'JetBrains Mono',ui-monospace,monospace;
  --sans:'Inter',system-ui,sans-serif;
}}
* {{ box-sizing:border-box; }}
body {{ margin:0; background:var(--surface-0); color:var(--text-primary);
  font-family:var(--sans); font-size:15px; line-height:1.5; }}
main {{ max-width:1040px; margin:0 auto; padding:32px 20px 64px; }}
h1 {{ font-size:22px; margin:0 0 4px; }}
h2 {{ font-size:17px; margin:0 0 2px; text-transform:capitalize; }}
.subtitle {{ color:var(--text-secondary); margin:0 0 8px; }}
.honesty {{ color:var(--text-muted); font-size:13px; margin:0 0 24px; }}
.card {{ background:var(--surface-1); border:1px solid var(--border);
  border-radius:10px; padding:20px 24px; margin-bottom:24px; }}
.desc {{ color:var(--text-secondary); margin:0 0 12px; font-size:13.5px; }}
.legend {{ display:flex; flex-wrap:wrap; gap:16px; align-items:center;
  font-size:12.5px; color:var(--text-secondary); margin-bottom:8px; }}
.key {{ display:inline-flex; align-items:center; gap:6px; }}
.chip {{ width:10px; height:10px; border-radius:3px; display:inline-block; }}
.chart {{ overflow-x:auto; }}
.chart svg {{ width:100%; height:auto; min-width:640px; display:block; }}
.axis {{ fill:#8b8a80; font-size:11px; font-family:var(--mono); }}
.dlabel {{ fill:#c3c2b7; font-size:11.5px; font-family:var(--sans); }}
.stats {{ display:grid; grid-template-columns:repeat(auto-fit,minmax(220px,1fr));
  gap:10px; margin-top:14px; }}
.stat {{ background:var(--surface-2); border:1px solid var(--border);
  border-radius:8px; padding:10px 12px; }}
.stat-label {{ color:var(--text-muted); font-size:11.5px; }}
.stat-value {{ font-size:14.5px; margin-top:2px; }}
.mono {{ font-family:var(--mono); }}
</style>
</head>
<body>
<main>
<h1>Even Keel — 24h simulation</h1>
<p class="subtitle">Usable-ratio trajectories with and without Even Keel, per traffic scenario. One tick = 5 simulated minutes; 288 ticks = 24h.</p>
<p class="honesty">Simulated traffic (MockNode, deterministic scripts and fees) driving the <em>real</em> planner, executor state machine, and store — the managed runs execute the same code paths as production. Policy: defaults (target 50%, 1 CKB/action, 10 CKB/day fee caps).</p>
{body}</main>
</body>
</html>
"#
    )
}
