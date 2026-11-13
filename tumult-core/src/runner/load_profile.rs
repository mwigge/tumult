//! The tool-agnostic half of the load adapter boundary.
//!
//! A [`LoadConfig`] (flat, ramp, spike or soak) is turned into one
//! [`LoadPlan`]: a starting concurrency plus a sequence of [`Stage`]s. Every
//! load adapter ([`super::k6`], [`super::oha`]) renders the same plan in its
//! own vocabulary — k6 as `--stage` arguments, oha as one process per
//! window — so a profile means the same thing whichever tool runs it.

use crate::types::{LoadConfig, LoadProfile};

/// Default virtual users when the config leaves `vus` unset.
pub const DEFAULT_VUS: u32 = 10;
/// Default duration in seconds when the config leaves `duration_s` unset.
pub const DEFAULT_DURATION_S: f64 = 30.0;

/// One segment of the plan: `target_vus` reached by the end of
/// `duration_s`. A `ramp` stage moves linearly from the previous level; a
/// step stage switches to `target_vus` at its start and holds it.
#[derive(Debug, Clone, PartialEq)]
pub struct Stage {
    pub duration_s: f64,
    pub target_vus: u32,
    pub ramp: bool,
}

/// The rendered shape of a load config.
#[derive(Debug, Clone, PartialEq)]
pub struct LoadPlan {
    /// Concurrency before the first stage applies.
    pub start_vus: u32,
    pub stages: Vec<Stage>,
    /// How often a streaming adapter should emit a sample; `None` means one
    /// sample per stage.
    pub sample_interval_s: Option<f64>,
}

impl LoadPlan {
    /// Total planned duration in seconds.
    #[must_use]
    pub fn total_duration_s(&self) -> f64 {
        self.stages.iter().map(|s| s.duration_s).sum()
    }

    /// The highest concurrency any stage reaches.
    #[must_use]
    pub fn peak_vus(&self) -> u32 {
        self.stages
            .iter()
            .map(|s| s.target_vus)
            .max()
            .unwrap_or(self.start_vus)
    }
}

/// Build the plan for `config`. Profiles are assumed validated by
/// `engine::validation`; a zero-length tail stage is simply dropped.
#[must_use]
pub fn plan(config: &LoadConfig) -> LoadPlan {
    let vus = config.vus.unwrap_or(DEFAULT_VUS);
    let duration_s = config.duration_s.unwrap_or(DEFAULT_DURATION_S);
    let step = |duration_s: f64, target_vus: u32| Stage {
        duration_s,
        target_vus,
        ramp: false,
    };
    let (start_vus, stages, sample_interval_s) = match &config.profile {
        None => (vus, vec![step(duration_s, vus)], None),
        Some(LoadProfile::Ramp {
            start_vus,
            ramp_up_s,
        }) => (
            *start_vus,
            vec![
                Stage {
                    duration_s: *ramp_up_s,
                    target_vus: vus,
                    ramp: true,
                },
                step(duration_s - ramp_up_s, vus),
            ],
            None,
        ),
        Some(LoadProfile::Spike {
            peak_vus,
            after_s,
            hold_s,
        }) => (
            vus,
            vec![
                step(*after_s, vus),
                step(*hold_s, *peak_vus),
                step(duration_s - after_s - hold_s, vus),
            ],
            None,
        ),
        Some(LoadProfile::Soak { sample_interval_s }) => {
            (vus, vec![step(duration_s, vus)], Some(*sample_interval_s))
        }
    };
    LoadPlan {
        start_vus,
        stages: stages.into_iter().filter(|s| s.duration_s > 0.0).collect(),
        sample_interval_s,
    }
}

/// Render a plan as k6 `--stage` values (`<duration>:<target>`), one per
/// entry. k6 ramps linearly from the previous target over each stage, so a
/// step stage whose level changes is rendered as a one-second jump followed
/// by the hold; a step at the current level is a single hold.
#[must_use]
pub fn k6_stages(plan: &LoadPlan) -> Vec<String> {
    const JUMP_S: f64 = 1.0;
    let mut level = plan.start_vus;
    let mut out = Vec::new();
    for stage in &plan.stages {
        let changes_level = stage.target_vus != level;
        if stage.ramp || !changes_level || stage.duration_s <= JUMP_S {
            out.push(k6_stage(stage.duration_s, stage.target_vus));
        } else {
            out.push(k6_stage(JUMP_S, stage.target_vus));
            out.push(k6_stage(stage.duration_s - JUMP_S, stage.target_vus));
        }
        level = stage.target_vus;
    }
    out
}

fn k6_stage(duration_s: f64, target_vus: u32) -> String {
    format!("{}s:{target_vus}", trim_seconds(duration_s))
}

/// Seconds without a trailing `.0` so k6 (Go durations) parses them; a
/// fractional value is kept as-is.
fn trim_seconds(s: f64) -> String {
    if s.fract() == 0.0 {
        format!("{s:.0}")
    } else {
        format!("{s}")
    }
}

/// One process invocation for an adapter that cannot shape traffic itself:
/// run at `vus` for `duration_s`, starting `offset_s` into the plan.
#[derive(Debug, Clone, PartialEq)]
pub struct Window {
    pub offset_s: f64,
    pub duration_s: f64,
    pub vus: u32,
}

/// Slice the plan into fixed windows of at most `sample_interval_s` (or
/// one window per stage when unset). A ramp stage interpolates the
/// concurrency at each window's midpoint, so the staircase approximates
/// the line; a step stage repeats its target.
#[must_use]
pub fn windows(plan: &LoadPlan) -> Vec<Window> {
    let mut out = Vec::new();
    let mut offset_s = 0.0;
    let mut level = plan.start_vus;
    for stage in &plan.stages {
        // A non-positive or non-finite interval would never advance the
        // loop; fall back to one window per stage.
        let width = plan
            .sample_interval_s
            .filter(|w| w.is_finite() && *w > 0.0)
            .unwrap_or(stage.duration_s)
            .min(stage.duration_s);
        let mut elapsed = 0.0;
        while elapsed < stage.duration_s {
            let duration_s = width.min(stage.duration_s - elapsed);
            let vus = if stage.ramp {
                interpolate(
                    level,
                    stage.target_vus,
                    (elapsed + duration_s / 2.0) / stage.duration_s,
                )
            } else {
                stage.target_vus
            };
            out.push(Window {
                offset_s: offset_s + elapsed,
                duration_s,
                vus,
            });
            elapsed += duration_s;
        }
        offset_s += stage.duration_s;
        level = stage.target_vus;
    }
    out
}

/// Linear interpolation between two concurrency levels at `t` in `0..=1`,
/// rounded to the nearest whole user and never below one.
fn interpolate(from: u32, to: u32, t: f64) -> u32 {
    let t = t.clamp(0.0, 1.0);
    let value = f64::from(from) + (f64::from(to) - f64::from(from)) * t;
    // Rounded value is within u32 range by construction (between `from` and `to`).
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let rounded = value.round() as u32;
    rounded.max(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::LoadTool;
    use std::collections::HashMap;

    fn config(
        profile: Option<LoadProfile>,
        vus: Option<u32>,
        duration_s: Option<f64>,
    ) -> LoadConfig {
        LoadConfig {
            tool: LoadTool::K6,
            script: "load.js".into(),
            vus,
            duration_s,
            thresholds: HashMap::new(),
            profile,
        }
    }

    fn approx(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn flat_config_is_one_step_with_defaults() {
        let p = plan(&config(None, None, None));
        assert_eq!(p.start_vus, DEFAULT_VUS);
        assert_eq!(p.stages.len(), 1);
        assert_eq!(p.stages[0].target_vus, DEFAULT_VUS);
        assert!(approx(p.stages[0].duration_s, DEFAULT_DURATION_S));
        assert!(!p.stages[0].ramp);
        assert_eq!(p.sample_interval_s, None);
        assert_eq!(k6_stages(&p), vec!["30s:10"]);
    }

    #[test]
    fn ramp_plan_ramps_then_holds() {
        let ramp = LoadProfile::Ramp {
            start_vus: 2,
            ramp_up_s: 30.0,
        };
        let p = plan(&config(Some(ramp), Some(50), Some(120.0)));
        assert_eq!(p.start_vus, 2);
        assert_eq!(
            p.stages,
            vec![
                Stage {
                    duration_s: 30.0,
                    target_vus: 50,
                    ramp: true
                },
                Stage {
                    duration_s: 90.0,
                    target_vus: 50,
                    ramp: false
                }
            ]
        );
        assert!(approx(p.total_duration_s(), 120.0));
        assert_eq!(p.peak_vus(), 50);
        assert_eq!(k6_stages(&p), vec!["30s:50", "90s:50"]);
    }

    #[test]
    fn ramp_that_fills_the_duration_drops_the_empty_hold() {
        let ramp = LoadProfile::Ramp {
            start_vus: 1,
            ramp_up_s: 60.0,
        };
        let p = plan(&config(Some(ramp), Some(20), Some(60.0)));
        assert_eq!(p.stages.len(), 1);
        assert!(p.stages[0].ramp);
    }

    #[test]
    fn spike_plan_steps_up_and_back() {
        let spike = LoadProfile::Spike {
            peak_vus: 200,
            after_s: 20.0,
            hold_s: 10.0,
        };
        let p = plan(&config(Some(spike), Some(10), Some(60.0)));
        assert_eq!(p.start_vus, 10);
        let levels: Vec<u32> = p.stages.iter().map(|s| s.target_vus).collect();
        assert_eq!(levels, vec![10, 200, 10]);
        let lengths: Vec<f64> = p.stages.iter().map(|s| s.duration_s).collect();
        assert_eq!(lengths, vec![20.0, 10.0, 30.0]);
        assert!(p.stages.iter().all(|s| !s.ramp));
        // k6 cannot step instantly: the level changes become a 1 s jump.
        assert_eq!(
            k6_stages(&p),
            vec!["20s:10", "1s:200", "9s:200", "1s:10", "29s:10"]
        );
    }

    #[test]
    fn soak_plan_is_one_long_step_with_a_sample_interval() {
        let soak = LoadProfile::Soak {
            sample_interval_s: 10.0,
        };
        let p = plan(&config(Some(soak), Some(20), Some(3600.0)));
        assert_eq!(p.stages.len(), 1);
        assert!(approx(p.stages[0].duration_s, 3600.0));
        assert_eq!(p.sample_interval_s, Some(10.0));
        assert_eq!(k6_stages(&p), vec!["3600s:20"]);
    }

    #[test]
    fn fractional_seconds_are_rendered_for_k6() {
        let p = plan(&config(None, Some(3), Some(1.5)));
        assert_eq!(k6_stages(&p), vec!["1.5s:3"]);
    }

    #[test]
    fn windows_slice_a_soak_by_the_sample_interval() {
        let soak = LoadProfile::Soak {
            sample_interval_s: 10.0,
        };
        let w = windows(&plan(&config(Some(soak), Some(20), Some(35.0))));
        assert_eq!(w.len(), 4);
        assert!(w.iter().all(|x| x.vus == 20));
        assert!(approx(w[3].offset_s, 30.0));
        assert!(approx(w[3].duration_s, 5.0));
    }

    #[test]
    fn windows_interpolate_a_ramp_and_repeat_a_step() {
        let ramp = LoadProfile::Ramp {
            start_vus: 0,
            ramp_up_s: 40.0,
        };
        let mut p = plan(&config(Some(ramp), Some(40), Some(60.0)));
        p.sample_interval_s = Some(10.0);
        let w = windows(&p);
        // Ramp: midpoints at 5, 15, 25, 35 s of a 40 s ramp to 40 users.
        let ramp_vus: Vec<u32> = w.iter().take(4).map(|x| x.vus).collect();
        assert_eq!(ramp_vus, vec![5, 15, 25, 35]);
        // Hold: two 10 s windows at the target.
        assert_eq!(w.len(), 6);
        assert!(w.iter().skip(4).all(|x| x.vus == 40));
        assert!(approx(w[5].offset_s, 50.0));
    }

    #[test]
    fn windows_fall_back_to_one_per_stage_on_a_bad_interval() {
        let mut p = plan(&config(None, Some(3), Some(30.0)));
        for bad in [0.0, -5.0, f64::NAN] {
            p.sample_interval_s = Some(bad);
            let w = windows(&p);
            assert_eq!(w.len(), 1, "interval {bad}");
            assert!(approx(w[0].duration_s, 30.0));
        }
    }

    #[test]
    fn windows_never_schedule_zero_users() {
        assert_eq!(interpolate(0, 10, 0.0), 1);
        assert_eq!(interpolate(0, 10, 1.0), 10);
        assert_eq!(interpolate(10, 0, 1.0), 1);
    }
}
