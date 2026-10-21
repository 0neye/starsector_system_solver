//! Regression coverage for evolutionary search cutoffs and environment settings.

use super::*;

fn test_state() -> State {
    let mut system = crate::system::System::new("Cutoff".to_string());
    system.add_planet(Planet::new(
        "Test".to_string(),
        FxHashMap::from_iter([
            ("hazard percent".to_string(), 100.0),
            ("ores".to_string(), 2.0),
        ]),
    ));
    State::new(crate::solver::Balance::new(10_000_000.0, 5, 5), system)
}

#[test]
fn nonfinite_polish_overrides_use_the_default() {
    let key = "SYSTEM_SOLVER_TEST_POLISH_FRAC";
    for value in ["NaN", "inf", "-inf", "invalid"] {
        std::env::set_var(key, value);
        assert_eq!(env_f64_opt(key).unwrap_or(0.40), 0.40, "{value}");
    }
    std::env::set_var(key, "0.25");
    assert_eq!(env_f64_opt(key), Some(0.25));
    std::env::remove_var(key);
}

#[test]
fn expired_batch_keeps_cached_scores_without_evaluating_misses() {
    let state = test_state();
    let floors = Goal::new(f64::NEG_INFINITY, None, None);
    let objective = Objective::Maximize {
        metric: Metric::Income,
        floors: &floors,
        horizon_months: 12,
    };
    let settings = SolverSettings::default();
    let cache = Mutex::new(FxHashMap::default());
    let cached_plan = SystemPlan::permit_all(&state);
    let cached_score = run_plan_cached(&state, &objective, &cached_plan, settings, &cache);
    let scores = score_plans(
        &state,
        &objective,
        settings,
        &cache,
        vec![SystemPlan::default(), cached_plan.clone()],
        Instant::now(),
        Duration::ZERO,
    );

    assert_eq!(
        cache.lock().unwrap().len(),
        1,
        "expired misses must not run"
    );
    assert_eq!(scores.len(), 1);
    assert_eq!(plan_key(&scores[0].0), plan_key(&cached_plan));
    assert_eq!(scores[0].1.log, cached_score.log);
    assert_eq!(scores[0].1.value, cached_score.value);
}

#[test]
#[ignore = "uses process-global cancellation, progress and EA settings; run alone"]
fn cancelled_surrogate_search_retains_an_accurate_incumbent() {
    // Cancel immediately after the accurate seed finishes, before any surrogate
    // population can complete. Confirmation must not start another simulation.
    std::env::set_var("SYSTEM_SOLVER_EA_SURROGATE", "1");
    assert!(ea_surrogate_enabled());
    let state = test_state();
    let ctx = SearchContext::new(&state);
    let floors = Goal::new(f64::NEG_INFINITY, None, None);
    let objective = Objective::Maximize {
        metric: Metric::Income,
        floors: &floors,
        horizon_months: 12,
    };
    let settings = SolverSettings::default();
    let seed = SystemPlan::permit_all(&state);
    let expected = run_plan(&state, &objective, &seed, settings);
    let cache = Mutex::new(FxHashMap::default());
    progress::set_sink(Box::new(|phase| {
        if phase == SolverPhase::Evolving {
            crate::solver::cancel::request();
        }
    }));
    let result = ea_search(
        &state,
        &ctx,
        &objective,
        settings,
        vec![seed.clone()],
        &cache,
        EaParams::get(),
        Instant::now(),
        Duration::from_secs(60),
    );
    progress::clear();
    crate::solver::cancel::clear();
    std::env::remove_var("SYSTEM_SOLVER_EA_SURROGATE");

    let (plan, score, _) = result.expect("completed accurate seed must survive cancellation");
    assert_eq!(plan_key(&plan), plan_key(&seed));
    assert_eq!(score.log, expected.log);
    assert_eq!(score.value, expected.value);
    let cache = cache.lock().unwrap();
    assert_eq!(cache.len(), 1, "cancel must prevent further simulations");
    assert!(cache.contains_key(&(plan_key(&seed), settings.facility_upgrade_placement)));
}
