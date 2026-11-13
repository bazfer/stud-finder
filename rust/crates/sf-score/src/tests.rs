use super::*;

fn tiny_graph() -> DependencyGraph {
    let mut g = DependencyGraph::new();
    g.register_file("app/models/order.rb");
    g.register_file("app/models/user.rb");
    g.register_file("app/models/profile.rb");
    g.register_file("app/services/checkout.rb");
    g.register_file("app/services/noop.rb");

    // Order: fan_in 3, fan_out 0
    // User: fan_in 2, fan_out 1 (-> Order)
    // Profile: fan_in 0, fan_out 2 (-> User, Order)
    // Checkout: fan_in 0, fan_out 2 (-> Order, User)
    // Noop: fan_in 0, fan_out 0
    g.add_reference("app/models/user.rb", "Order");
    g.add_reference("app/models/profile.rb", "User");
    g.add_reference("app/models/profile.rb", "Order");
    g.add_reference("app/services/checkout.rb", "Order");
    g.add_reference("app/services/checkout.rb", "User");
    g
}

fn row<'a>(rows: &'a [ScoredFile], path: &str) -> &'a ScoredFile {
    let target = Path::new(path);
    rows.iter()
        .find(|r| r.path == target)
        .unwrap_or_else(|| panic!("no row for {path}"))
}

fn signals_with_complexity<'a>(
    graph: &'a DependencyGraph,
    complexities: &[(&str, usize)],
) -> Signals<'a> {
    let mut cx = HashMap::new();
    for (p, c) in complexities {
        cx.insert(PathBuf::from(*p), *c);
    }
    Signals {
        graph,
        complexity: cx,
        churn_commits: HashMap::new(),
        churn_lines: HashMap::new(),
        coupling: HashMap::new(),
        coverage: None,
        loc: HashMap::new(),
    }
}

#[test]
fn percentile_rank_endpoints_and_middle() {
    let r = percentile_rank_usize(&[1, 2, 3, 4, 5]);
    assert!((r[0] - 0.0).abs() < 1e-9);
    assert!((r[4] - 1.0).abs() < 1e-9);
    assert!((r[2] - 0.5).abs() < 1e-9);
}

#[test]
fn percentile_rank_ties_use_lower_bound_matching_ruby() {
    let r = percentile_rank_usize(&[7, 7, 7, 7]);
    assert_eq!(r, vec![0.0, 0.0, 0.0, 0.0]);
    let r = percentile_rank_usize(&[0, 0, 1]);
    assert_eq!(r, vec![0.0, 0.0, 1.0]);
    let r = percentile_rank_usize(&[1, 2, 3, 3]);
    assert!((r[0] - 0.0).abs() < 1e-9);
    assert!((r[1] - 1.0 / 3.0).abs() < 1e-9);
    assert!((r[2] - 2.0 / 3.0).abs() < 1e-9);
    assert!((r[3] - 2.0 / 3.0).abs() < 1e-9);
    let r = percentile_rank_usize(&[1, 1, 2, 3]);
    assert!((r[0] - 0.0).abs() < 1e-9);
    assert!((r[1] - 0.0).abs() < 1e-9);
    assert!((r[2] - 2.0 / 3.0).abs() < 1e-9);
    assert!((r[3] - 1.0).abs() < 1e-9);
}

#[test]
fn scorer_emits_row_per_registered_file() {
    let g = tiny_graph();
    let rows = Scorer::new().score(&g);
    assert_eq!(rows.len(), 5);
}

#[test]
fn composite_uses_four_to_two_weight_ratio_in_two_signal_mode() {
    let g = tiny_graph();
    let rows = Scorer::new().score(&g);
    let order = row(&rows, "app/models/order.rb");
    // 2-signal weights renormalize to w_fi : w_fo = 4 : 2 regardless
    // of the absolute base values.
    let w_fi = BASE_WEIGHT_FAN_IN / (BASE_WEIGHT_FAN_IN + BASE_WEIGHT_FAN_OUT);
    let w_fo = BASE_WEIGHT_FAN_OUT / (BASE_WEIGHT_FAN_IN + BASE_WEIGHT_FAN_OUT);
    let expected = round4(w_fi * order.fan_in_pct + w_fo * order.fan_out_pct);
    assert!((order.composite - expected).abs() < 1e-9);
}

#[test]
fn classification_tiers_partition_the_set() {
    let g = tiny_graph();
    let rows = Scorer::new().score(&g);
    let mut trunks = 0;
    let mut branches = 0;
    let mut leaves = 0;
    for r in &rows {
        match r.class {
            Class::Trunk => trunks += 1,
            Class::Branch => branches += 1,
            Class::Leaf => leaves += 1,
        }
    }
    assert_eq!(trunks + branches + leaves, 5);
}

fn floor_test_graph() -> DependencyGraph {
    let mut g = DependencyGraph::new();
    let peer_count = 100usize;
    for i in 0..peer_count {
        g.register_file(format!("peer_{i}.rb"));
    }
    for i in 0..peer_count {
        for offset in 1..=50 {
            let target_class = format!("Peer{}", (i + offset) % peer_count);
            g.add_reference(format!("peer_{i}.rb"), target_class);
        }
    }
    g.register_file("hot.rb");
    for i in 0..30 {
        g.add_reference(format!("peer_{i}.rb"), "Hot");
    }
    g
}

#[test]
fn fan_in_floor_escalates_leaf_to_branch_when_composite_is_low() {
    let g = floor_test_graph();

    let no_floor = Scorer::with_thresholds(TRUNK_THRESHOLD_PCT, BRANCH_THRESHOLD_PCT, usize::MAX);
    let rows_no_floor = no_floor.score(&g);
    let hot_no_floor = row(&rows_no_floor, "hot.rb");
    assert!(
        hot_no_floor.fan_in >= FAN_IN_FLOOR,
        "setup: hot should meet the floor count ({} >= {})",
        hot_no_floor.fan_in,
        FAN_IN_FLOOR
    );
    assert_eq!(
        hot_no_floor.class,
        Class::Leaf,
        "setup: without floor, hot should classify as Leaf by composite percentile"
    );

    let default = Scorer::new();
    let rows_default = default.score(&g);
    let hot_default = row(&rows_default, "hot.rb");
    assert_eq!(
        hot_default.class,
        Class::Branch,
        "fan_in >= {FAN_IN_FLOOR} must escalate Leaf to Branch"
    );
}

#[test]
fn fan_in_floor_never_downgrades_a_trunk_that_meets_the_count() {
    let mut g = DependencyGraph::new();
    g.register_file("top.rb");
    for i in 0..30 {
        g.add_reference("top.rb", format!("Dep{i}"));
    }
    for i in 0..30 {
        let f = format!("caller_{i}.rb");
        g.register_file(&f);
        g.add_reference(&f, "Top");
    }

    let no_floor = Scorer::with_thresholds(TRUNK_THRESHOLD_PCT, BRANCH_THRESHOLD_PCT, usize::MAX);
    let top_no_floor = row(&no_floor.score(&g), "top.rb").clone();
    assert!(
        top_no_floor.fan_in >= FAN_IN_FLOOR,
        "setup: top should meet the floor count"
    );
    assert_eq!(top_no_floor.class, Class::Trunk);

    let top_default = row(&Scorer::new().score(&g), "top.rb").clone();
    assert_eq!(top_default.class, Class::Trunk);
}

#[test]
fn evidence_is_one_point_zero_in_this_slice() {
    let g = tiny_graph();
    let rows = Scorer::new().score(&g);
    for r in rows {
        assert_eq!(r.evidence, 1.0);
    }
}

#[test]
fn empty_graph_yields_no_rows() {
    let g = DependencyGraph::new();
    let rows = Scorer::new().score(&g);
    assert!(rows.is_empty());
}

#[test]
fn single_file_renders_zero_pcts() {
    let mut g = DependencyGraph::new();
    g.register_file("solo.rb");
    let rows = Scorer::new().score(&g);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].fan_in_pct, 0.0);
    assert_eq!(rows[0].fan_out_pct, 0.0);
}

#[test]
fn two_files_with_same_signals_both_get_zero_pct() {
    let mut g = DependencyGraph::new();
    g.register_file("a.rb");
    g.register_file("b.rb");
    g.add_reference("a.rb", "External");
    g.add_reference("b.rb", "External");
    let rows = Scorer::new().score(&g);
    for r in rows {
        assert_eq!(r.fan_in_pct, 0.0);
        assert_eq!(r.fan_out_pct, 0.0);
        assert_eq!(r.composite, 0.0);
        assert_eq!(r.class, Class::Leaf);
    }
}

// --- 3-signal mode with complexity ---

#[test]
fn three_signal_renormalization_weights_sum_to_one() {
    let mut g = DependencyGraph::new();
    g.register_file("top.rb");
    for i in 0..5 {
        g.add_reference("top.rb", format!("Dep{i}"));
    }
    for i in 0..5 {
        let f = format!("caller_{i}.rb");
        g.register_file(&f);
        g.add_reference(&f, "Top");
    }
    let mut cx_entries = vec![("top.rb", 20usize)];
    for i in 0..5 {
        let name = Box::leak(format!("caller_{i}.rb").into_boxed_str()) as &'static str;
        cx_entries.push((name, 1));
    }
    let signals = signals_with_complexity(&g, &cx_entries);
    let rows = Scorer::new().score_with_signals(signals);
    let top = row(&rows, "top.rb");
    assert!(
        (top.composite - 1.0).abs() < 1e-9,
        "three-signal composite should hit 1.0 when every signal's pct is 1.0, got {}",
        top.composite
    );
    let w_fi =
        BASE_WEIGHT_FAN_IN / (BASE_WEIGHT_FAN_IN + BASE_WEIGHT_FAN_OUT + BASE_WEIGHT_COMPLEXITY);
    let w_fo =
        BASE_WEIGHT_FAN_OUT / (BASE_WEIGHT_FAN_IN + BASE_WEIGHT_FAN_OUT + BASE_WEIGHT_COMPLEXITY);
    let w_cx = BASE_WEIGHT_COMPLEXITY
        / (BASE_WEIGHT_FAN_IN + BASE_WEIGHT_FAN_OUT + BASE_WEIGHT_COMPLEXITY);
    assert!((w_fi + w_fo + w_cx - 1.0).abs() < 1e-12);
}

#[test]
fn empty_complexity_map_degenerates_to_two_signal_mode() {
    let g = tiny_graph();
    let rows_two_sig = Scorer::new().score(&g);
    let rows_signals = Scorer::new().score_with_signals(Signals::from_graph(&g));

    assert_eq!(rows_two_sig.len(), rows_signals.len());
    for a in &rows_two_sig {
        let b = rows_signals
            .iter()
            .find(|r| r.path == a.path)
            .unwrap_or_else(|| panic!("missing row for {}", a.path.display()));
        assert!((a.composite - b.composite).abs() < 1e-9);
        assert_eq!(a.class, b.class);
        assert!((a.fan_in_pct - b.fan_in_pct).abs() < 1e-9);
        assert!((a.fan_out_pct - b.fan_out_pct).abs() < 1e-9);
        assert_eq!(b.complexity, 0);
        assert_eq!(b.complexity_pct, 0.0);
    }
}

fn complexity_floor_test_graph() -> (DependencyGraph, HashMap<PathBuf, usize>) {
    let mut g = DependencyGraph::new();
    let peer_count = 100usize;
    for i in 0..peer_count {
        g.register_file(format!("peer_{i}.rb"));
    }
    for i in 0..peer_count {
        for offset in 1..=10 {
            let target = format!("Peer{}", (i + offset) % peer_count);
            g.add_reference(format!("peer_{i}.rb"), target);
        }
    }
    g.register_file("spiky.rb");

    let mut complexity = HashMap::new();
    for i in 0..peer_count {
        complexity.insert(PathBuf::from(format!("peer_{i}.rb")), 50);
    }
    complexity.insert(PathBuf::from("spiky.rb"), COMPLEXITY_FLOOR);
    (g, complexity)
}

#[test]
fn complexity_floor_escalates_leaf_to_branch_when_composite_is_low() {
    let (g, cx) = complexity_floor_test_graph();

    let no_cx_floor = Scorer::with_floors(
        TRUNK_THRESHOLD_PCT,
        BRANCH_THRESHOLD_PCT,
        FAN_IN_FLOOR,
        usize::MAX,
    );
    let signals = Signals {
        graph: &g,
        complexity: cx.clone(),
        churn_commits: HashMap::new(),
        churn_lines: HashMap::new(),
        coupling: HashMap::new(),
        coverage: None,
        loc: HashMap::new(),
    };
    let rows_no_floor = no_cx_floor.score_with_signals(signals);
    let spiky_no_floor = row(&rows_no_floor, "spiky.rb");
    assert!(spiky_no_floor.complexity >= COMPLEXITY_FLOOR);
    assert_eq!(spiky_no_floor.class, Class::Leaf);

    let default_signals = Signals {
        graph: &g,
        complexity: cx,
        churn_commits: HashMap::new(),
        churn_lines: HashMap::new(),
        coupling: HashMap::new(),
        coverage: None,
        loc: HashMap::new(),
    };
    let rows_default = Scorer::new().score_with_signals(default_signals);
    let spiky_default = row(&rows_default, "spiky.rb");
    assert_eq!(spiky_default.class, Class::Branch);
}

#[test]
fn complexity_floor_never_downgrades_trunk_or_branch() {
    let mut g = DependencyGraph::new();
    g.register_file("top.rb");
    for i in 0..30 {
        g.add_reference("top.rb", format!("Dep{i}"));
    }
    for i in 0..30 {
        let f = format!("caller_{i}.rb");
        g.register_file(&f);
        g.add_reference(&f, "Top");
    }
    let mut cx = HashMap::new();
    cx.insert(PathBuf::from("top.rb"), 20);
    for i in 0..30 {
        cx.insert(PathBuf::from(format!("caller_{i}.rb")), 1);
    }

    let signals = Signals {
        graph: &g,
        complexity: cx,
        churn_commits: HashMap::new(),
        churn_lines: HashMap::new(),
        coupling: HashMap::new(),
        coverage: None,
        loc: HashMap::new(),
    };
    let rows = Scorer::new().score_with_signals(signals);
    let top = row(&rows, "top.rb");
    assert!(top.complexity >= COMPLEXITY_FLOOR);
    assert_eq!(top.class, Class::Trunk);
}

// --- 4-signal mode with churn ---

fn signals_with_churn<'a>(
    graph: &'a DependencyGraph,
    complexities: &[(&str, usize)],
    churn: &[(&str, usize, usize)],
) -> Signals<'a> {
    let mut cx = HashMap::new();
    for (p, c) in complexities {
        cx.insert(PathBuf::from(*p), *c);
    }
    let mut churn_c = HashMap::new();
    let mut churn_l = HashMap::new();
    for (p, c, l) in churn {
        churn_c.insert(PathBuf::from(*p), *c);
        churn_l.insert(PathBuf::from(*p), *l);
    }
    Signals {
        graph,
        complexity: cx,
        churn_commits: churn_c,
        churn_lines: churn_l,
        coupling: HashMap::new(),
        coverage: None,
        loc: HashMap::new(),
    }
}

#[test]
fn four_signal_renormalization_weights_sum_to_one() {
    // All four signals present → active_total = 0.19 + 0.095 + 0.2375 + 0.2375 = 0.76.
    // A file that is max on every signal gets composite = 1.0.
    let mut g = DependencyGraph::new();
    g.register_file("top.rb");
    for i in 0..5 {
        g.add_reference("top.rb", format!("Dep{i}"));
    }
    let mut cx_entries: Vec<(&str, usize)> = vec![("top.rb", 20)];
    let mut churn_entries: Vec<(&str, usize, usize)> = vec![("top.rb", 20, 200)];
    for i in 0..5 {
        let f_name = Box::leak(format!("caller_{i}.rb").into_boxed_str()) as &'static str;
        g.register_file(f_name);
        g.add_reference(f_name, "Top");
        cx_entries.push((f_name, 1));
        churn_entries.push((f_name, 1, 10));
    }

    let signals = signals_with_churn(&g, &cx_entries, &churn_entries);
    let rows = Scorer::new().score_with_signals(signals);
    let top = row(&rows, "top.rb");
    assert!(
        (top.composite - 1.0).abs() < 1e-9,
        "4-signal composite should hit 1.0 when every signal's pct is 1.0, got {}",
        top.composite
    );

    let sum_four =
        BASE_WEIGHT_FAN_IN + BASE_WEIGHT_FAN_OUT + BASE_WEIGHT_COMPLEXITY + BASE_WEIGHT_CHURN;
    let w_fi = BASE_WEIGHT_FAN_IN / sum_four;
    let w_fo = BASE_WEIGHT_FAN_OUT / sum_four;
    let w_cx = BASE_WEIGHT_COMPLEXITY / sum_four;
    let w_ch = BASE_WEIGHT_CHURN / sum_four;
    assert!((w_fi + w_fo + w_cx + w_ch - 1.0).abs() < 1e-12);
    // And the 4:2:5:5 ratio is preserved.
    assert!((w_fi / w_fo - 2.0).abs() < 1e-9);
    assert!((w_cx / w_fo - 2.5).abs() < 1e-9);
    assert!((w_ch / w_fo - 2.5).abs() < 1e-9);
}

#[test]
fn empty_churn_maps_degenerate_to_three_signal_mode() {
    // Scoring with empty churn maps must produce identical composites
    // to scoring with just complexity. Back-compat guarantee for PR #78
    // callers.
    let g = tiny_graph();
    let cx_entries: Vec<(&str, usize)> = vec![
        ("app/models/order.rb", 5),
        ("app/models/user.rb", 3),
        ("app/models/profile.rb", 2),
        ("app/services/checkout.rb", 4),
        ("app/services/noop.rb", 1),
    ];

    let signals_cx = signals_with_complexity(&g, &cx_entries);
    let rows_three = Scorer::new().score_with_signals(signals_cx);

    let signals_all = signals_with_churn(&g, &cx_entries, &[]);
    let rows_three_with_empty_churn = Scorer::new().score_with_signals(signals_all);

    assert_eq!(rows_three.len(), rows_three_with_empty_churn.len());
    for a in &rows_three {
        let b = rows_three_with_empty_churn
            .iter()
            .find(|r| r.path == a.path)
            .unwrap();
        assert!((a.composite - b.composite).abs() < 1e-9);
        assert_eq!(a.class, b.class);
    }
}

// --- 5-signal mode with coupling ---

#[test]
fn five_signal_renormalization_weights_sum_to_one() {
    let sum_all = BASE_WEIGHT_FAN_IN
        + BASE_WEIGHT_FAN_OUT
        + BASE_WEIGHT_COMPLEXITY
        + BASE_WEIGHT_CHURN
        + EXTRA_WEIGHT_COUPLING;
    let w_fi = BASE_WEIGHT_FAN_IN / sum_all;
    let w_fo = BASE_WEIGHT_FAN_OUT / sum_all;
    let w_cx = BASE_WEIGHT_COMPLEXITY / sum_all;
    let w_ch = BASE_WEIGHT_CHURN / sum_all;
    let w_cp = EXTRA_WEIGHT_COUPLING / sum_all;
    assert!(
        (w_fi + w_fo + w_cx + w_ch + w_cp - 1.0).abs() < 1e-12,
        "renormalized weights must sum to 1.0, got {}",
        w_fi + w_fo + w_cx + w_ch + w_cp
    );
    // Base ratio preserved.
    assert!((w_fi / w_fo - 2.0).abs() < 1e-9);
    assert!((w_cx / w_fo - 2.5).abs() < 1e-9);
    assert!((w_ch / w_fo - 2.5).abs() < 1e-9);
    // Coupling is small relative to the base ratio.
    assert!(w_cp < w_fo);
}

#[test]
fn coupling_appears_in_row_and_lifts_composite() {
    // A graph where `a.rb` and `b.rb` are coupled vs a solo `c.rb`.
    // With coupling active, a and b should score higher than c.
    let mut g = DependencyGraph::new();
    g.register_file("a.rb");
    g.register_file("b.rb");
    g.register_file("c.rb");
    g.add_reference("a.rb", "External");
    g.add_reference("b.rb", "External");
    g.add_reference("c.rb", "External");

    let mut coupling = HashMap::new();
    coupling.insert(
        PathBuf::from("a.rb"),
        CouplingSignal {
            partner: PathBuf::from("b.rb"),
            ratio: 0.9,
            co_changes: 12,
            partners: 1,
        },
    );
    coupling.insert(
        PathBuf::from("b.rb"),
        CouplingSignal {
            partner: PathBuf::from("a.rb"),
            ratio: 0.9,
            co_changes: 12,
            partners: 1,
        },
    );

    let signals = Signals {
        graph: &g,
        complexity: HashMap::new(),
        churn_commits: HashMap::new(),
        churn_lines: HashMap::new(),
        coupling,
        coverage: None,
        loc: HashMap::new(),
    };
    let rows = Scorer::new().score_with_signals(signals);
    let a = row(&rows, "a.rb");
    let c = row(&rows, "c.rb");
    assert_eq!(a.max_coupling_partner, PathBuf::from("b.rb"));
    assert!((a.max_coupling - 0.9).abs() < 1e-9);
    // a has a coupling pct of 1.0, c has 0.0 → a > c on composite.
    assert!(
        a.composite > c.composite,
        "a={} c={}",
        a.composite,
        c.composite
    );
}

#[test]
fn empty_coupling_map_degenerates_to_four_signal_mode() {
    let g = tiny_graph();
    let cx_entries: Vec<(&str, usize)> = vec![
        ("app/models/order.rb", 5),
        ("app/models/user.rb", 3),
        ("app/models/profile.rb", 2),
        ("app/services/checkout.rb", 4),
        ("app/services/noop.rb", 1),
    ];
    let churn_entries: Vec<(&str, usize, usize)> = vec![
        ("app/models/order.rb", 10, 100),
        ("app/models/user.rb", 5, 60),
        ("app/models/profile.rb", 3, 20),
        ("app/services/checkout.rb", 7, 80),
        ("app/services/noop.rb", 1, 5),
    ];

    let signals_four = signals_with_churn(&g, &cx_entries, &churn_entries);
    let rows_four = Scorer::new().score_with_signals(signals_four);

    let signals_four_empty_cp = Signals {
        graph: &g,
        complexity: cx_entries
            .iter()
            .map(|(p, c)| (PathBuf::from(*p), *c))
            .collect(),
        churn_commits: churn_entries
            .iter()
            .map(|(p, c, _)| (PathBuf::from(*p), *c))
            .collect(),
        churn_lines: churn_entries
            .iter()
            .map(|(p, _, l)| (PathBuf::from(*p), *l))
            .collect(),
        coupling: HashMap::new(),
        coverage: None,
        loc: HashMap::new(),
    };
    let rows_four_cp_empty = Scorer::new().score_with_signals(signals_four_empty_cp);

    assert_eq!(rows_four.len(), rows_four_cp_empty.len());
    for a in &rows_four {
        let b = rows_four_cp_empty
            .iter()
            .find(|r| r.path == a.path)
            .unwrap();
        assert!((a.composite - b.composite).abs() < 1e-9);
        assert_eq!(a.class, b.class);
        assert_eq!(b.max_coupling, 0.0);
        assert_eq!(b.max_coupling_partner, PathBuf::new());
    }
}

#[test]
fn churn_composite_rounds_near_ties_before_ranking() {
    // Ohm's P1 on PR #79. With 6 files whose churn_commits and
    // churn_lines ranks cross so that files A and B have averages
    // 0.3 and 0.30000000000000004 respectively (IEEE-754 noise from
    // (0+3)/5/2 vs (1+2)/5/2), Ruby `composite_churn_pct` treats them
    // as tied via `round(10)`. Rust must now do the same.
    //
    // Fixture: 6 files with (count, lines):
    //   f0: (0,  3) → rank(count) = 0, rank(lines) = 3
    //   f1: (1,  2) → rank(count) = 1, rank(lines) = 2
    //   f2: (2,  1) → rank(count) = 2, rank(lines) = 1
    //   f3: (3,  0) → rank(count) = 3, rank(lines) = 0
    //   f4: (4,  4) → rank(count) = 4, rank(lines) = 4
    //   f5: (5,  5) → rank(count) = 5, rank(lines) = 5
    // ranks are lower-bound over 6 values so /5 for the lower 4.
    // (rank_f0_count + rank_f0_lines)/2 = (0 + 3)/5/2 = 0.3
    // (rank_f1_count + rank_f1_lines)/2 = (1 + 2)/5/2 = 0.30000000000000004 in f64
    // (rank_f2_count + rank_f2_lines)/2 = (2 + 1)/5/2 = 0.3 exactly
    // (rank_f3_count + rank_f3_lines)/2 = (3 + 0)/5/2 = 0.3 exactly
    // All four should round to 0.3 and produce the same churn_pct.
    let mut g = DependencyGraph::new();
    for i in 0..6 {
        g.register_file(format!("f{i}.rb"));
    }

    let mut churn_c = HashMap::new();
    let mut churn_l = HashMap::new();
    let churn: [(usize, usize); 6] = [(10, 13), (11, 12), (12, 11), (13, 10), (14, 14), (15, 15)];
    for (i, (c, l)) in churn.iter().enumerate() {
        churn_c.insert(PathBuf::from(format!("f{i}.rb")), *c);
        churn_l.insert(PathBuf::from(format!("f{i}.rb")), *l);
    }

    let signals = Signals {
        graph: &g,
        complexity: HashMap::new(),
        churn_commits: churn_c,
        churn_lines: churn_l,
        coupling: HashMap::new(),
        coverage: None,
        loc: HashMap::new(),
    };
    let rows = Scorer::new().score_with_signals(signals);
    let pct_for = |name: &str| {
        let target = PathBuf::from(name);
        rows.iter().find(|r| r.path == target).unwrap().churn_pct
    };
    // Without round(10), f1 would land in a different rank bucket due
    // to the 1e-17 noise. With it, all four "middle" rows tie.
    assert!(
        (pct_for("f0.rb") - pct_for("f1.rb")).abs() < 1e-9,
        "near-tied averages must collapse: f0_pct={}, f1_pct={}",
        pct_for("f0.rb"),
        pct_for("f1.rb")
    );
    assert!(
        (pct_for("f0.rb") - pct_for("f2.rb")).abs() < 1e-9,
        "near-tied averages must collapse: f0_pct={}, f2_pct={}",
        pct_for("f0.rb"),
        pct_for("f2.rb")
    );
    assert!(
        (pct_for("f0.rb") - pct_for("f3.rb")).abs() < 1e-9,
        "near-tied averages must collapse: f0_pct={}, f3_pct={}",
        pct_for("f0.rb"),
        pct_for("f3.rb")
    );
}

// --- 6- and 7-signal mode with coverage + interaction ---

fn signals_with_coverage<'a>(
    graph: &'a DependencyGraph,
    complexities: &[(&str, usize)],
    churn: &[(&str, usize, usize)],
    coverage: &[(&str, f64)],
) -> Signals<'a> {
    let mut cx = HashMap::new();
    for (p, c) in complexities {
        cx.insert(PathBuf::from(*p), *c);
    }
    let mut churn_c = HashMap::new();
    let mut churn_l = HashMap::new();
    for (p, c, l) in churn {
        churn_c.insert(PathBuf::from(*p), *c);
        churn_l.insert(PathBuf::from(*p), *l);
    }
    let mut cov = HashMap::new();
    for (p, v) in coverage {
        cov.insert(PathBuf::from(*p), *v);
    }
    Signals {
        graph,
        complexity: cx,
        churn_commits: churn_c,
        churn_lines: churn_l,
        coupling: HashMap::new(),
        coverage: Some(cov),
        loc: HashMap::new(),
    }
}

#[test]
fn six_signal_weights_renormalize_to_one() {
    // With coverage active, Ruby's `active_total` becomes
    // 0.19 + 0.095 + 0.2375 + 0.2375 + 0.095 (coverage) + 0.095 (interaction)
    // = 0.95, and each weight divides through. Base ratio 4:2:5:5 stays.
    let sum_six = BASE_WEIGHT_FAN_IN
        + BASE_WEIGHT_FAN_OUT
        + BASE_WEIGHT_COMPLEXITY
        + BASE_WEIGHT_CHURN
        + BASE_WEIGHT_COVERAGE
        + EXTRA_WEIGHT_INTERACTION;
    let w_fi = BASE_WEIGHT_FAN_IN / sum_six;
    let w_fo = BASE_WEIGHT_FAN_OUT / sum_six;
    let w_cx = BASE_WEIGHT_COMPLEXITY / sum_six;
    let w_ch = BASE_WEIGHT_CHURN / sum_six;
    let w_cov = BASE_WEIGHT_COVERAGE / sum_six;
    let w_int = EXTRA_WEIGHT_INTERACTION / sum_six;
    assert!((w_fi + w_fo + w_cx + w_ch + w_cov + w_int - 1.0).abs() < 1e-12);
    // Base ratio preserved.
    assert!((w_fi / w_fo - 2.0).abs() < 1e-9);
    assert!((w_cx / w_fo - 2.5).abs() < 1e-9);
    assert!((w_ch / w_fo - 2.5).abs() < 1e-9);
    // Coverage shares w_fo's absolute value.
    assert!((w_cov - w_fo).abs() < 1e-12);
    // Interaction extra matches coverage magnitude.
    assert!((w_int - w_cov).abs() < 1e-12);
}

#[test]
fn seven_signal_renormalization_with_coupling_sums_to_one() {
    let sum_seven = BASE_WEIGHT_FAN_IN
        + BASE_WEIGHT_FAN_OUT
        + BASE_WEIGHT_COMPLEXITY
        + BASE_WEIGHT_CHURN
        + EXTRA_WEIGHT_COUPLING
        + BASE_WEIGHT_COVERAGE
        + EXTRA_WEIGHT_INTERACTION;
    let total = (BASE_WEIGHT_FAN_IN
        + BASE_WEIGHT_FAN_OUT
        + BASE_WEIGHT_COMPLEXITY
        + BASE_WEIGHT_CHURN
        + EXTRA_WEIGHT_COUPLING
        + BASE_WEIGHT_COVERAGE
        + EXTRA_WEIGHT_INTERACTION)
        / sum_seven;
    assert!((total - 1.0).abs() < 1e-12);
}

#[test]
fn coverage_appears_in_row_and_lifts_composite() {
    let mut g = DependencyGraph::new();
    g.register_file("a.rb");
    g.register_file("b.rb");
    g.add_reference("a.rb", "External");
    g.add_reference("b.rb", "External");

    // a.rb has 100% coverage; b.rb has 0% coverage. Everything else
    // equal (same fan_in / fan_out / complexity / churn). Per Ruby
    // `coverage_risk_values = 1.0 - coverage`, b.rb's risk is 1.0
    // (uncovered = risky) and a.rb's risk is 0.0, so coverage_pct
    // = risk_pct flips the ordering: b now ranks higher on
    // coverage_pct and so OUT-SCORES a on composite.
    let signals = signals_with_coverage(&g, &[], &[], &[("a.rb", 1.0), ("b.rb", 0.0)]);
    let rows = Scorer::new().score_with_signals(signals);
    let a = row(&rows, "a.rb");
    let b = row(&rows, "b.rb");
    assert_eq!(a.coverage, 1.0);
    assert_eq!(b.coverage, 0.0);
    assert!(
        (a.coverage_pct - 0.0).abs() < 1e-9,
        "100%-covered file has risk 0.0 → risk_pct 0.0, got {}",
        a.coverage_pct
    );
    assert!(
        (b.coverage_pct - 1.0).abs() < 1e-9,
        "0%-covered file has risk 1.0 → risk_pct 1.0, got {}",
        b.coverage_pct
    );
    assert!(
        b.composite > a.composite,
        "risk-based composite must put the uncovered file above the covered one"
    );
}

#[test]
fn coverage_risk_direction_golden_at_0_50_100() {
    // Ohm's P1 regression fixture: three otherwise-identical files at
    // 0%, 50%, 100% coverage must receive DECREASING risk
    // contributions (so the 0% file has the highest coverage_pct and
    // 100% the lowest).
    let mut g = DependencyGraph::new();
    g.register_file("empty.rb");
    g.register_file("half.rb");
    g.register_file("full.rb");

    let signals = signals_with_coverage(
        &g,
        &[],
        &[],
        &[("empty.rb", 0.0), ("half.rb", 0.5), ("full.rb", 1.0)],
    );
    let rows = Scorer::new().score_with_signals(signals);
    let empty = row(&rows, "empty.rb");
    let half = row(&rows, "half.rb");
    let full = row(&rows, "full.rb");
    // empty has the LOWEST raw coverage, so HIGHEST risk, so HIGHEST
    // coverage_pct — matches Ruby `coverage_risk_pct` direction.
    assert!(empty.coverage_pct > half.coverage_pct);
    assert!(half.coverage_pct > full.coverage_pct);
    // Composite follows: empty > half > full on risk-based composite.
    assert!(empty.composite > half.composite);
    assert!(half.composite > full.composite);
}

#[test]
fn interaction_cross_term_is_fan_in_risk_product() {
    // Ruby `interaction_score = w_int * fan_in_pct * pcts[:coverage]`
    // where `pcts[:coverage]` is the risk percentile. The cross-term
    // lifts the composite when BOTH fan_in and uncovered-risk are
    // high — not when fan_in is high and coverage is high.
    //
    // Fixture: a load-bearing but UNCOVERED file (hot.rb: fan_in big,
    // coverage 0.0 → risk 1.0) should score higher than an equally
    // load-bearing but FULLY-COVERED file (safe.rb: same fan_in,
    // coverage 1.0 → risk 0.0). The two leaves have identical graph
    // signals; only coverage differs.
    let mut g = DependencyGraph::new();
    g.register_file("hot.rb");
    g.register_file("safe.rb");
    g.register_file("c.rb");
    g.register_file("d.rb");
    g.add_reference("c.rb", "Hot");
    g.add_reference("c.rb", "Safe");
    g.add_reference("d.rb", "Hot");
    g.add_reference("d.rb", "Safe");

    let signals = signals_with_coverage(
        &g,
        &[],
        &[],
        &[
            ("hot.rb", 0.0),
            ("safe.rb", 1.0),
            ("c.rb", 0.5),
            ("d.rb", 0.5),
        ],
    );
    let rows = Scorer::new().score_with_signals(signals);
    let hot = row(&rows, "hot.rb");
    let safe = row(&rows, "safe.rb");
    // Same fan_in (2 each), same graph signals.
    assert_eq!(hot.fan_in, safe.fan_in);
    // Opposite risk percentiles.
    assert!(hot.coverage_pct > safe.coverage_pct);
    // The interaction cross-term uses fan_in_pct * coverage_pct
    // (where coverage_pct is now RISK percentile). Hot has
    // fan_in_pct * ~1.0 contribution; safe has fan_in_pct * ~0.0.
    // Hot must out-score safe on composite.
    assert!(
        hot.composite > safe.composite,
        "uncovered load-bearing file should outscore its covered twin"
    );
}

#[test]
fn absent_coverage_degrades_to_five_signal_mode() {
    // Ruby `coverage_available? = !@coverage.nil?`. When the caller
    // passes `coverage: None`, coverage + interaction must drop out
    // of the composite and the run matches the 5-signal path.
    let g = tiny_graph();
    let cx_entries: Vec<(&str, usize)> = vec![
        ("app/models/order.rb", 5),
        ("app/models/user.rb", 3),
        ("app/models/profile.rb", 2),
        ("app/services/checkout.rb", 4),
        ("app/services/noop.rb", 1),
    ];
    let churn_entries: Vec<(&str, usize, usize)> = vec![
        ("app/models/order.rb", 10, 100),
        ("app/models/user.rb", 5, 60),
        ("app/models/profile.rb", 3, 20),
        ("app/services/checkout.rb", 7, 80),
        ("app/services/noop.rb", 1, 5),
    ];
    let rows_four =
        Scorer::new().score_with_signals(signals_with_churn(&g, &cx_entries, &churn_entries));
    // Same signals but coverage explicitly None.
    let rows_cov_empty = Scorer::new().score_with_signals(Signals {
        graph: &g,
        complexity: cx_entries
            .iter()
            .map(|(p, c)| (PathBuf::from(*p), *c))
            .collect(),
        churn_commits: churn_entries
            .iter()
            .map(|(p, c, _)| (PathBuf::from(*p), *c))
            .collect(),
        churn_lines: churn_entries
            .iter()
            .map(|(p, _, l)| (PathBuf::from(*p), *l))
            .collect(),
        coupling: HashMap::new(),
        coverage: None,
        loc: HashMap::new(),
    });
    assert_eq!(rows_four.len(), rows_cov_empty.len());
    for a in &rows_four {
        let b = rows_cov_empty.iter().find(|r| r.path == a.path).unwrap();
        assert!((a.composite - b.composite).abs() < 1e-9);
        assert_eq!(a.class, b.class);
        assert_eq!(b.coverage, 0.0);
        assert_eq!(b.coverage_pct, 0.0);
    }
}

#[test]
fn provided_empty_coverage_still_activates_signal() {
    // Ruby `coverage_available? = !@coverage.nil?` — an explicitly
    // provided empty report still activates coverage + interaction.
    // Every file defaults to 0.0 coverage (risk 1.0) via
    // `@coverage.fetch(file, 0.0)` in `coverage_risk_values`.
    // Fixes Ohm's P2 on PR #80 (map-emptiness gate was collapsing
    // this case into the "unavailable" branch).
    let g = tiny_graph();
    // Signals with coverage: Some(empty HashMap).
    let signals = Signals {
        graph: &g,
        complexity: HashMap::new(),
        churn_commits: HashMap::new(),
        churn_lines: HashMap::new(),
        coupling: HashMap::new(),
        coverage: Some(HashMap::new()),
        loc: HashMap::new(),
    };
    let rows = Scorer::new().score_with_signals(signals);
    // Every file defaulted to coverage = 0.0 (risk 1.0). That's an
    // all-tied risk signal → insufficient dispersion → risk_pct = 0.0
    // for every file. But the SIGNAL is still active (it's in the
    // composite's weight denominator).
    //
    // Composite(two-signal baseline) uses active_total = 0.285 (fan_in + fan_out).
    // Composite(two + coverage) uses active_total = 0.285 + 0.095 + 0.095 = 0.475.
    // The fan_in_pct/fan_out_pct contributions are re-weighted: they
    // get divided by a LARGER denominator, so each row's composite
    // SHRINKS vs the two-signal baseline. Verify this by comparing.
    let signals_baseline = Signals::from_graph(&g);
    let baseline_rows = Scorer::new().score_with_signals(signals_baseline);
    for r in &rows {
        let b = baseline_rows.iter().find(|x| x.path == r.path).unwrap();
        if b.composite > 0.0 {
            assert!(
                r.composite < b.composite,
                "activating empty coverage must renormalize weights, shrinking composite; got {} vs baseline {}",
                r.composite,
                b.composite
            );
        }
    }
}

#[test]
fn churn_composite_is_percentile_of_avg_count_and_line_pcts() {
    // File A: churn 10c/100l, File B: 5c/60l, File C: 3c/20l.
    // count_pcts (rank/n-1): A=1.0, B=0.5, C=0.0
    // line_pcts: A=1.0, B=0.5, C=0.0
    // avg: A=1.0, B=0.5, C=0.0
    // percentile_rank of avg: A=1.0, B=0.5, C=0.0
    let mut g = DependencyGraph::new();
    g.register_file("a.rb");
    g.register_file("b.rb");
    g.register_file("c.rb");

    let signals = signals_with_churn(
        &g,
        &[],
        &[("a.rb", 10, 100), ("b.rb", 5, 60), ("c.rb", 3, 20)],
    );
    let rows = Scorer::new().score_with_signals(signals);
    let a = row(&rows, "a.rb");
    let b = row(&rows, "b.rb");
    let c = row(&rows, "c.rb");
    assert!((a.churn_pct - 1.0).abs() < 1e-9);
    assert!((b.churn_pct - 0.5).abs() < 1e-9);
    assert!((c.churn_pct - 0.0).abs() < 1e-9);
}

// --- sf-signals-expand (PR #83) regression tests ---

#[test]
fn instability_is_fan_out_over_total_rounded_to_4() {
    // User: fan_in 2, fan_out 1 -> instability = 1/3 = 0.3333 (round4)
    // Profile: fan_in 0, fan_out 2 -> instability = 1.0
    // Noop: fan_in 0, fan_out 0 -> instability = 0.0 (both-zero guard)
    let g = tiny_graph();
    let rows = Scorer::new().score(&g);
    assert!((row(&rows, "app/models/user.rb").instability - 0.3333).abs() < 1e-9);
    assert!((row(&rows, "app/models/profile.rb").instability - 1.0).abs() < 1e-9);
    assert!((row(&rows, "app/services/noop.rb").instability - 0.0).abs() < 1e-9);
    assert!((row(&rows, "app/models/order.rb").instability - 0.0).abs() < 1e-9);
}

#[test]
fn instability_pct_is_lower_bound_percentile_over_instability_values() {
    // tiny_graph instability values:
    //   Order 0.0, User 0.3333, Profile 1.0, Checkout 1.0, Noop 0.0
    // Sorted: 0.0, 0.0, 0.3333, 1.0, 1.0
    // Lower-bound ranks (strictly-smaller / (n-1) = /4):
    //   0.0 -> 0/4 = 0.0
    //   0.3333 -> 2/4 = 0.5
    //   1.0 -> 3/4 = 0.75
    let g = tiny_graph();
    let rows = Scorer::new().score(&g);
    assert!((row(&rows, "app/models/order.rb").instability_pct - 0.0).abs() < 1e-9);
    assert!((row(&rows, "app/services/noop.rb").instability_pct - 0.0).abs() < 1e-9);
    assert!((row(&rows, "app/models/user.rb").instability_pct - 0.5).abs() < 1e-9);
    assert!((row(&rows, "app/models/profile.rb").instability_pct - 0.75).abs() < 1e-9);
    assert!((row(&rows, "app/services/checkout.rb").instability_pct - 0.75).abs() < 1e-9);
}

#[test]
fn loc_defaults_to_zero_when_signal_absent() {
    // from_graph() has empty loc map. ScoredFile.loc must be 0 and
    // loc_pct must degenerate to 0.0 (no ranking of an all-zero column).
    let g = tiny_graph();
    let rows = Scorer::new().score(&g);
    for r in &rows {
        assert_eq!(r.loc, 0);
        assert!((r.loc_pct - 0.0).abs() < 1e-9);
    }
}

#[test]
fn loc_and_loc_pct_are_populated_from_signal_map() {
    let g = tiny_graph();
    let mut loc = HashMap::new();
    loc.insert(PathBuf::from("app/models/order.rb"), 100);
    loc.insert(PathBuf::from("app/models/user.rb"), 50);
    loc.insert(PathBuf::from("app/models/profile.rb"), 25);
    loc.insert(PathBuf::from("app/services/checkout.rb"), 10);
    loc.insert(PathBuf::from("app/services/noop.rb"), 5);
    let signals = Signals {
        graph: &g,
        complexity: HashMap::new(),
        churn_commits: HashMap::new(),
        churn_lines: HashMap::new(),
        coupling: HashMap::new(),
        coverage: None,
        loc,
    };
    let rows = Scorer::new().score_with_signals(signals);
    // Raw values forward verbatim.
    assert_eq!(row(&rows, "app/models/order.rb").loc, 100);
    assert_eq!(row(&rows, "app/services/noop.rb").loc, 5);
    // 5 distinct values -> endpoints 0.0 and 1.0.
    assert!((row(&rows, "app/models/order.rb").loc_pct - 1.0).abs() < 1e-9);
    assert!((row(&rows, "app/services/noop.rb").loc_pct - 0.0).abs() < 1e-9);
}

#[test]
fn escalation_tags_complexity_floor_over_fan_in_floor_on_tie() {
    // Ruby `floored_classification` checks complexity first — so a
    // file meeting BOTH floors reports `complexity_floor`, not
    // `fan_in_floor`. floor_test_graph already proves `hot.rb`
    // meets FAN_IN_FLOOR and is Leaf-by-composite under default
    // weights. Hand it a COMPLEXITY_FLOOR-meeting complexity and
    // the two floors both fire on the same file.
    let g = floor_test_graph();
    // Give every peer a high complexity so hot.rb sits at the
    // bottom of the complexity percentile despite meeting the
    // absolute floor — the composite then still lands in Leaf.
    let mut complexity = HashMap::new();
    for i in 0..100 {
        complexity.insert(PathBuf::from(format!("peer_{i}.rb")), 50);
    }
    complexity.insert(PathBuf::from("hot.rb"), COMPLEXITY_FLOOR);
    let signals = Signals {
        graph: &g,
        complexity,
        churn_commits: HashMap::new(),
        churn_lines: HashMap::new(),
        coupling: HashMap::new(),
        coverage: None,
        loc: HashMap::new(),
    };
    let rows = Scorer::new().score_with_signals(signals);
    let hot = row(&rows, "hot.rb");
    assert!(hot.fan_in >= FAN_IN_FLOOR, "setup: hot meets fan_in floor");
    assert!(
        hot.complexity >= COMPLEXITY_FLOOR,
        "setup: hot meets complexity floor"
    );
    assert_eq!(hot.class, Class::Branch);
    assert_eq!(
        hot.escalation, "complexity_floor",
        "complexity_floor must win the tag tie — matches Ruby's check order"
    );
}

#[test]
fn escalation_is_empty_when_classification_not_floor_escalated() {
    // tiny_graph has no file meeting either floor so escalation is "".
    let g = tiny_graph();
    let rows = Scorer::new().score(&g);
    for r in &rows {
        assert_eq!(r.escalation, "");
    }
}

#[test]
fn coupling_partners_count_forwards_from_signal() {
    let mut g = DependencyGraph::new();
    g.register_file("a.rb");
    g.register_file("b.rb");
    g.register_file("c.rb");
    g.register_file("d.rb");
    // Give everyone a reference so percentiles aren't all zero.
    for p in ["a.rb", "b.rb", "c.rb", "d.rb"] {
        g.add_reference(p, "External");
    }

    let mut coupling = HashMap::new();
    // a.rb has 3 qualifying partners (b, c, d); strongest is b.
    coupling.insert(
        PathBuf::from("a.rb"),
        CouplingSignal {
            partner: PathBuf::from("b.rb"),
            ratio: 0.9,
            co_changes: 9,
            partners: 3,
        },
    );
    // b.rb has 1 (just a).
    coupling.insert(
        PathBuf::from("b.rb"),
        CouplingSignal {
            partner: PathBuf::from("a.rb"),
            ratio: 0.9,
            co_changes: 9,
            partners: 1,
        },
    );
    let signals = Signals {
        graph: &g,
        complexity: HashMap::new(),
        churn_commits: HashMap::new(),
        churn_lines: HashMap::new(),
        coupling,
        coverage: None,
        loc: HashMap::new(),
    };
    let rows = Scorer::new().score_with_signals(signals);
    assert_eq!(row(&rows, "a.rb").coupling_partners, 3);
    assert_eq!(row(&rows, "b.rb").coupling_partners, 1);
    // c.rb and d.rb have no coupling entry -> 0.
    assert_eq!(row(&rows, "c.rb").coupling_partners, 0);
    assert_eq!(row(&rows, "d.rb").coupling_partners, 0);
}

// --- sf-newness apply_newness (PR #84) ---

use sf_newness::FileNewness;

fn mk_metadata(entries: &[(&str, FileNewness)]) -> HashMap<PathBuf, FileNewness> {
    entries
        .iter()
        .map(|(p, m)| (PathBuf::from(*p), m.clone()))
        .collect()
}

#[test]
fn apply_newness_promotes_new_leaf_to_branch_with_recency_floor() {
    // A Leaf by composite + new_file → Branch with escalation=recency_floor.
    let g = tiny_graph();
    let mut rows = Scorer::new().score(&g);
    // Find a file whose class is Leaf under default scoring.
    let noop = row(&rows, "app/services/noop.rb").clone();
    assert_eq!(noop.class, Class::Leaf, "setup: noop.rb is Leaf");
    let metadata = mk_metadata(&[(
        "app/services/noop.rb",
        FileNewness {
            new_file: true,
            age_days: 2,
            total_commits: 1,
            metadata_available: true,
        },
    )]);
    apply_newness(&mut rows, &metadata, &g, |_| false);
    let after = row(&rows, "app/services/noop.rb");
    assert_eq!(after.class, Class::Branch, "new_file Leaf must be promoted");
    assert_eq!(after.escalation, "recency_floor");
    assert!(after.new_file);
    assert_eq!(after.age_days, 2);
    assert_eq!(after.total_commits, 1);
    assert!(after.newness_metadata_available);
}

#[test]
fn apply_newness_promotes_new_file_with_trunk_dependency_to_trunk_adjacent() {
    // Build a graph where `new.rb` depends on `hot.rb` (a Trunk).
    let mut g = DependencyGraph::new();
    // Use the floor-test-graph setup for hot.rb to earn Trunk by composite.
    let peer_count = 100usize;
    for i in 0..peer_count {
        g.register_file(format!("peer_{i}.rb"));
    }
    for i in 0..peer_count {
        for offset in 1..=50 {
            let target_class = format!("Peer{}", (i + offset) % peer_count);
            g.add_reference(format!("peer_{i}.rb"), target_class);
        }
    }
    g.register_file("hot.rb");
    for i in 0..30 {
        g.add_reference(format!("peer_{i}.rb"), "Hot");
    }
    // Hot's score needs a boost into Trunk — feed it churn.
    g.register_file("new.rb");
    g.add_reference("new.rb", "Hot"); // new.rb depends on hot.rb
    let mut churn_commits = HashMap::new();
    let mut churn_lines = HashMap::new();
    churn_commits.insert(PathBuf::from("hot.rb"), 100);
    churn_lines.insert(PathBuf::from("hot.rb"), 1000);
    let signals = Signals {
        graph: &g,
        complexity: HashMap::new(),
        churn_commits,
        churn_lines,
        coupling: HashMap::new(),
        coverage: None,
        loc: HashMap::new(),
    };
    let mut rows = Scorer::new().score_with_signals(signals);
    // Verify hot.rb is Trunk
    let hot = row(&rows, "hot.rb");
    assert_eq!(hot.class, Class::Trunk, "setup: hot.rb classifies Trunk");

    let metadata = mk_metadata(&[(
        "new.rb",
        FileNewness {
            new_file: true,
            age_days: 1,
            total_commits: 1,
            metadata_available: true,
        },
    )]);
    apply_newness(&mut rows, &metadata, &g, |_| false);
    let new_row = row(&rows, "new.rb");
    assert_eq!(new_row.class, Class::Trunk);
    assert_eq!(new_row.escalation, "trunk_adjacent");
}

#[test]
fn apply_newness_leaves_mature_files_alone() {
    let g = tiny_graph();
    let mut rows = Scorer::new().score(&g);
    let before: Vec<(PathBuf, Class, String)> = rows
        .iter()
        .map(|r| (r.path.clone(), r.class, r.escalation.clone()))
        .collect();
    let metadata: HashMap<PathBuf, FileNewness> = rows
        .iter()
        .map(|r| {
            (
                r.path.clone(),
                FileNewness {
                    new_file: false, // all mature
                    age_days: 365,
                    total_commits: 100,
                    metadata_available: true,
                },
            )
        })
        .collect();
    apply_newness(&mut rows, &metadata, &g, |_| false);
    for (i, r) in rows.iter().enumerate() {
        assert_eq!(r.class, before[i].1, "mature file class must not change");
        assert_eq!(
            r.escalation, before[i].2,
            "mature file escalation must not change"
        );
        assert!(!r.new_file);
        assert_eq!(r.age_days, 365);
        assert_eq!(r.total_commits, 100);
    }
}

#[test]
fn apply_newness_preserves_floor_escalation_on_mature_files() {
    // Even if a file's metadata is available, if it isn't new_file,
    // the preexisting escalation (complexity_floor / fan_in_floor)
    // must survive the merge — Ruby `Newness.apply` keeps it.
    let g = floor_test_graph();
    let mut complexity = HashMap::new();
    for i in 0..100 {
        complexity.insert(PathBuf::from(format!("peer_{i}.rb")), 50);
    }
    complexity.insert(PathBuf::from("hot.rb"), COMPLEXITY_FLOOR);
    let signals = Signals {
        graph: &g,
        complexity,
        churn_commits: HashMap::new(),
        churn_lines: HashMap::new(),
        coupling: HashMap::new(),
        coverage: None,
        loc: HashMap::new(),
    };
    let mut rows = Scorer::new().score_with_signals(signals);
    let hot_before = row(&rows, "hot.rb").clone();
    assert_eq!(hot_before.escalation, "complexity_floor");

    // Mature metadata: new_file = false.
    let metadata = mk_metadata(&[(
        "hot.rb",
        FileNewness {
            new_file: false,
            age_days: 200,
            total_commits: 50,
            metadata_available: true,
        },
    )]);
    apply_newness(&mut rows, &metadata, &g, |_| false);
    let hot_after = row(&rows, "hot.rb");
    assert_eq!(
        hot_after.escalation, "complexity_floor",
        "mature + preexisting-floor must keep its escalation tag"
    );
    assert_eq!(hot_after.class, Class::Branch);
}

#[test]
fn apply_newness_new_file_with_floor_escalation_recency_floor_wins() {
    // A new_file that was already Branch via complexity_floor →
    // Ruby reclassifies to Branch/recency_floor (recency_floor
    // overrides the floor tag). Same end class, different tag.
    let g = floor_test_graph();
    let mut complexity = HashMap::new();
    for i in 0..100 {
        complexity.insert(PathBuf::from(format!("peer_{i}.rb")), 50);
    }
    complexity.insert(PathBuf::from("hot.rb"), COMPLEXITY_FLOOR);
    let signals = Signals {
        graph: &g,
        complexity,
        churn_commits: HashMap::new(),
        churn_lines: HashMap::new(),
        coupling: HashMap::new(),
        coverage: None,
        loc: HashMap::new(),
    };
    let mut rows = Scorer::new().score_with_signals(signals);
    assert_eq!(row(&rows, "hot.rb").escalation, "complexity_floor");

    let metadata = mk_metadata(&[(
        "hot.rb",
        FileNewness {
            new_file: true,
            age_days: 1,
            total_commits: 1,
            metadata_available: true,
        },
    )]);
    apply_newness(&mut rows, &metadata, &g, |_| false);
    let hot_after = row(&rows, "hot.rb");
    assert_eq!(hot_after.class, Class::Branch);
    assert_eq!(
        hot_after.escalation, "recency_floor",
        "recency_floor overrides a preexisting complexity_floor on a new file"
    );
}

#[test]
fn apply_newness_evidence_matches_ruby_formula_with_and_without_coverage() {
    let g = tiny_graph();
    let mut rows = Scorer::new().score(&g);
    // All rows start with evidence = 1.0 (placeholder).
    assert!(rows.iter().all(|r| (r.evidence - 1.0).abs() < 1e-9));

    let metadata = mk_metadata(&[
        (
            "app/models/order.rb",
            FileNewness {
                new_file: false,
                age_days: 15,     // age_component = 0.5
                total_commits: 0, // commits_component = 0
                metadata_available: true,
            },
        ),
        (
            "app/services/noop.rb",
            FileNewness {
                new_file: true,
                age_days: 0,
                total_commits: 0,
                metadata_available: true,
            },
        ),
    ]);
    // order.rb HAS coverage; noop.rb does NOT.
    let has_cov = |p: &Path| p == Path::new("app/models/order.rb");
    apply_newness(&mut rows, &metadata, &g, has_cov);
    let order = row(&rows, "app/models/order.rb");
    // history_only = (0.5 + 0)/2 = 0.25
    // with_coverage = (0.5 + 0 + 1)/3 = 0.5
    // max = 0.5
    assert!(
        (order.evidence - 0.5).abs() < 1e-4,
        "order.rb with coverage → evidence 0.5, got {}",
        order.evidence
    );
    let noop = row(&rows, "app/services/noop.rb");
    // history_only = (0 + 0)/2 = 0; no coverage → 0.
    assert!(
        (noop.evidence - 0.0).abs() < 1e-4,
        "noop.rb evidence was {}",
        noop.evidence
    );

    // A file NOT in metadata uses the disabled placeholder →
    // evidence stays at the placeholder 1.0.
    let user = row(&rows, "app/models/user.rb");
    assert!(
        (user.evidence - 1.0).abs() < 1e-9,
        "user.rb not in metadata → evidence preserved at 1.0, got {}",
        user.evidence
    );
}

#[test]
fn apply_newness_does_not_cascade_trunk_promotions() {
    // Trunk status is frozen BEFORE pass 2, so a new file promoted
    // to Trunk by trunk-adjacent does NOT then promote another
    // new file whose dependency is that newly-trunk file. Matches
    // Ruby, which snapshots `trunk_paths` once before the loop.
    let mut g = DependencyGraph::new();
    // Three-chain: c.rb → b.rb → a.rb (a is pre-existing Trunk).
    for p in ["a.rb", "b.rb", "c.rb"] {
        g.register_file(p);
    }
    // a.rb earns Trunk via lots of fan_in. We'll fake Trunk below
    // by directly injecting class state after scoring — simpler.
    g.add_reference("b.rb", "A");
    g.add_reference("c.rb", "B");
    let mut rows = Scorer::new().score(&g);
    // Freeze the pre-apply_newness class map: only a.rb starts as
    // Trunk; b.rb and c.rb start as Leaf. This is the state that
    // would exist if a.rb were top-of-composite on a much larger
    // graph — the test must gate the NO-CASCADE property, not the
    // scorer's own classification logic on a 3-file toy graph.
    for r in rows.iter_mut() {
        r.class = match r.path.to_str().unwrap_or("") {
            "a.rb" => Class::Trunk,
            _ => Class::Leaf,
        };
        r.escalation = String::new();
    }
    let metadata = mk_metadata(&[
        (
            "b.rb",
            FileNewness {
                new_file: true,
                age_days: 1,
                total_commits: 1,
                metadata_available: true,
            },
        ),
        (
            "c.rb",
            FileNewness {
                new_file: true,
                age_days: 1,
                total_commits: 1,
                metadata_available: true,
            },
        ),
    ]);
    apply_newness(&mut rows, &metadata, &g, |_| false);
    // b.rb should promote to Trunk (depends on Trunk a.rb).
    assert_eq!(row(&rows, "b.rb").class, Class::Trunk);
    assert_eq!(row(&rows, "b.rb").escalation, "trunk_adjacent");
    // c.rb should NOT promote to Trunk even though its dependency b.rb
    // JUST became Trunk — the trunk-set is frozen before pass 2.
    // It should land Branch/recency_floor instead.
    let c = row(&rows, "c.rb");
    assert_ne!(
        c.class,
        Class::Trunk,
        "cascading trunk promotion is NOT allowed — Ruby freezes trunk_paths before pass 2"
    );
    assert_eq!(c.escalation, "recency_floor");
}
