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
        },
    );
    coupling.insert(
        PathBuf::from("b.rb"),
        CouplingSignal {
            partner: PathBuf::from("a.rb"),
            ratio: 0.9,
            co_changes: 12,
        },
    );

    let signals = Signals {
        graph: &g,
        complexity: HashMap::new(),
        churn_commits: HashMap::new(),
        churn_lines: HashMap::new(),
        coupling,
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
