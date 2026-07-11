# frozen_string_literal: true

require 'spec_helper'
require 'stud_finder/newness'
require 'stud_finder/scorer'

RSpec.describe StudFinder::Scorer do
  let(:files) { %w[a.rb b.rb c.rb d.rb] }
  let(:fan_in) { { 'a.rb' => 3, 'b.rb' => 2, 'c.rb' => 1, 'd.rb' => 0 } }
  let(:fan_out) { { 'a.rb' => 0, 'b.rb' => 1, 'c.rb' => 2, 'd.rb' => 0 } }
  let(:complexity) { { 'a.rb' => 1, 'b.rb' => 10, 'c.rb' => 0, 'd.rb' => 0 } }
  let(:churn) { { 'a.rb' => 0, 'b.rb' => 1, 'c.rb' => 10, 'd.rb' => 0 } }

  def scorer(**overrides)
    options = {
      files: files,
      fan_in: fan_in,
      fan_out: fan_out,
      complexity: complexity,
      churn: churn,
      branch_threshold: 50,
      trunk_threshold: 85
    }.merge(overrides)

    described_class.new(**options)
  end

  it 'renormalizes uniform rebalance weights (four-factor) to sum to 1.0 without coverage' do
    weights = scorer.normalized_weights

    expect(weights[:fan_in]).to be_within(0.0001).of(0.25)
    expect(weights[:fan_out]).to be_within(0.0001).of(0.125)
    expect(weights[:complexity]).to be_within(0.0001).of(0.3125)
    expect(weights[:churn]).to be_within(0.0001).of(0.3125)
    expect(weights[:coverage]).to be_nil
    expect(weights.values.compact.sum).to be_within(0.0001).of(1.0)
  end

  it 'produces scores in the inclusive 0.0 to 1.0 range' do
    scores = scorer.call.map { |row| row[:score] }

    expect(scores).to all(be_between(0.0, 1.0).inclusive)
  end

  it 'uses the uniform rebalance four-factor distribution when coverage is unavailable' do
    rows = scorer.call.to_h { |row| [row[:path], row] }

    expect(rows['b.rb'][:score]).to eq(0.7708)
    expect(rows['b.rb'][:coverage]).to be_nil
  end

  it 'classifies by composite score' do
    rows = scorer.call.to_h { |row| [row[:path], row] }

    expect(rows['a.rb'][:classification]).to eq('leaf')
    expect(rows['b.rb'][:classification]).to eq('branch')
    expect(rows['c.rb'][:classification]).to eq('branch')
  end

  it 'escalates a composite leaf to branch when raw complexity reaches the absolute floor' do
    files = %w[target.rb peer.rb]
    rows = described_class.new(
      files: files,
      fan_in: files.to_h { |file| [file, 0] },
      fan_out: files.to_h { |file| [file, 0] },
      complexity: files.to_h { |file| [file, 20] },
      churn: files.to_h { |file| [file, 0] }
    ).call.to_h { |row| [row[:path], row] }

    expect(rows['target.rb'][:score]).to eq(0.0)
    expect(rows['target.rb'][:classification]).to eq('branch')
  end

  it 'escalates a composite leaf to branch when raw fan_in reaches the absolute floor' do
    files = %w[target.rb peer.rb]
    rows = described_class.new(
      files: files,
      fan_in: files.to_h { |file| [file, 30] },
      fan_out: files.to_h { |file| [file, 0] },
      complexity: files.to_h { |file| [file, 0] },
      churn: files.to_h { |file| [file, 0] }
    ).call.to_h { |row| [row[:path], row] }

    expect(rows['target.rb'][:score]).to eq(0.0)
    expect(rows['target.rb'][:classification]).to eq('branch')
  end

  it 'keeps a composite leaf as leaf when raw complexity and fan_in are under the absolute floors' do
    files = %w[target.rb peer.rb]
    rows = described_class.new(
      files: files,
      fan_in: files.to_h { |file| [file, 24] },
      fan_out: files.to_h { |file| [file, 0] },
      complexity: files.to_h { |file| [file, 14] },
      churn: files.to_h { |file| [file, 0] }
    ).call.to_h { |row| [row[:path], row] }

    expect(rows['target.rb'][:score]).to eq(0.0)
    expect(rows['target.rb'][:classification]).to eq('leaf')
  end

  it 'does not demote a composite trunk when raw complexity and fan_in are below the absolute floors' do
    files = %w[trunk.rb low.rb]
    rows = described_class.new(
      files: files,
      fan_in: { 'trunk.rb' => 2, 'low.rb' => 0 },
      fan_out: { 'trunk.rb' => 5, 'low.rb' => 0 },
      complexity: { 'trunk.rb' => 3, 'low.rb' => 0 },
      churn: { 'trunk.rb' => 5, 'low.rb' => 0 },
      weights: { fan_in: 0.0, fan_out: 0.5, complexity: 0.0, churn: 0.5, coverage: 0.0 },
      trunk_threshold: 85
    ).call.to_h { |row| [row[:path], row] }

    expect(rows['trunk.rb'][:score]).to eq(1.0)
    expect(rows['trunk.rb'][:classification]).to eq('trunk')
  end

  it 'lets a branch from the complexity floor stack with newness trunk-adjacent escalation' do
    files = %w[trunk.rb new_client.rb]
    rows = described_class.new(
      files: files,
      fan_in: files.to_h { |file| [file, 0] },
      fan_out: { 'trunk.rb' => 5, 'new_client.rb' => 0 },
      complexity: { 'trunk.rb' => 0, 'new_client.rb' => 20 },
      churn: { 'trunk.rb' => 5, 'new_client.rb' => 0 },
      weights: { fan_in: 0.0, fan_out: 0.5, complexity: 0.0, churn: 0.5, coverage: 0.0 },
      trunk_threshold: 85
    ).call
    metadata = {
      'trunk.rb' => { new_file: false, age_days: 120, total_commits: 4, metadata_available: true },
      'new_client.rb' => { new_file: true, age_days: 1, total_commits: 1, metadata_available: true }
    }
    edges = { 'new_client.rb' => { dependencies: ['trunk.rb'] } }

    result = StudFinder::Newness.apply(rows: rows, edges: edges, metadata: metadata).to_h { |row| [row[:path], row] }

    expect(rows.to_h { |row| [row[:path], row] }['new_client.rb'][:classification]).to eq('branch')
    expect(result['new_client.rb'][:classification]).to eq('trunk')
    expect(result['new_client.rb'][:escalation]).to eq('trunk_adjacent')
  end

  it 'classifies max complexity, max churn, zero coverage, and median fan_in by interaction-adjusted score' do
    files = %w[low.rb risky.rb fan_in_top.rb]
    rows = described_class.new(
      files: files,
      fan_in: { 'low.rb' => 0, 'risky.rb' => 1, 'fan_in_top.rb' => 2 },
      fan_out: { 'low.rb' => 0, 'risky.rb' => 5, 'fan_in_top.rb' => 1 },
      complexity: { 'low.rb' => 0, 'risky.rb' => 10, 'fan_in_top.rb' => 1 },
      churn: { 'low.rb' => 0, 'risky.rb' => 10, 'fan_in_top.rb' => 1 },
      coverage: { 'low.rb' => 1.0, 'risky.rb' => 0.0, 'fan_in_top.rb' => 1.0 },
      weights: { fan_in: 0.20, fan_out: 0.10, complexity: 0.25, churn: 0.25, coverage: 0.10, interaction: 0.10 },
      trunk_threshold: 84
    ).call.to_h { |row| [row[:path], row] }

    expect(rows['risky.rb'][:fan_in_pct]).to eq(0.5)
    expect(rows['risky.rb'][:score]).to eq(0.85)
    expect(rows['risky.rb'][:classification]).to eq('trunk')
  end

  it 'classifies max fan_in with median complexity, churn, and coverage as trunk when score reaches threshold' do
    files = %w[low.rb fan_in_top.rb high.rb]
    rows = described_class.new(
      files: files,
      fan_in: { 'low.rb' => 0, 'fan_in_top.rb' => 3, 'high.rb' => 1 },
      fan_out: { 'low.rb' => 0, 'fan_in_top.rb' => 5, 'high.rb' => 1 },
      complexity: { 'low.rb' => 0, 'fan_in_top.rb' => 1, 'high.rb' => 2 },
      churn: { 'low.rb' => 0, 'fan_in_top.rb' => 1, 'high.rb' => 2 },
      coverage: { 'low.rb' => 1.0, 'fan_in_top.rb' => 0.5, 'high.rb' => 0.0 },
      weights: { fan_in: 0.20, fan_out: 0.10, complexity: 0.25, churn: 0.25, coverage: 0.10, interaction: 0.10 },
      branch_threshold: 30,
      trunk_threshold: 65
    ).call.to_h { |row| [row[:path], row] }

    expect(rows['fan_in_top.rb'][:fan_in_pct]).to eq(1.0)
    expect(rows['fan_in_top.rb'][:complexity_pct]).to eq(0.5)
    expect(rows['fan_in_top.rb'][:churn_pct]).to eq(0.5)
    expect(rows['fan_in_top.rb'][:score]).to eq(0.65)
    expect(rows['fan_in_top.rb'][:classification]).to eq('trunk')
  end

  it 'applies custom trunk and branch thresholds to composite score' do
    files = %w[leaf.rb branch.rb near_trunk.rb trunk.rb]
    rows = described_class.new(
      files: files,
      fan_in: { 'leaf.rb' => 0, 'branch.rb' => 0, 'near_trunk.rb' => 1, 'trunk.rb' => 2 },
      fan_out: { 'leaf.rb' => 0, 'branch.rb' => 1, 'near_trunk.rb' => 2, 'trunk.rb' => 3 },
      complexity: { 'leaf.rb' => 0, 'branch.rb' => 0, 'near_trunk.rb' => 2, 'trunk.rb' => 3 },
      churn: { 'leaf.rb' => 0, 'branch.rb' => 1, 'near_trunk.rb' => 2, 'trunk.rb' => 3 },
      coverage: { 'leaf.rb' => 1.0, 'branch.rb' => 0.75, 'near_trunk.rb' => 0.25, 'trunk.rb' => 0.0 },
      weights: { fan_in: 0.20, fan_out: 0.10, complexity: 0.25, churn: 0.25, coverage: 0.10, interaction: 0.10 },
      branch_threshold: 30,
      trunk_threshold: 90
    ).call.to_h { |row| [row[:path], row] }

    expect(rows['trunk.rb'][:score]).to eq(1.0)
    expect(rows['trunk.rb'][:classification]).to eq('trunk')
    expect(rows['near_trunk.rb'][:score]).to eq(0.6444)
    expect(rows['near_trunk.rb'][:classification]).to eq('branch')
    expect(rows['leaf.rb'][:classification]).to eq('leaf')
  end

  it 'uses five-factor scoring when coverage is provided' do
    rows = scorer(
      coverage: { 'a.rb' => 1.0, 'b.rb' => 0.5, 'c.rb' => 0.0, 'd.rb' => 0.25 },
      weights: { fan_in: 0.20, fan_out: 0.10, complexity: 0.25, churn: 0.25, coverage: 0.10, interaction: 0.10 }
    ).call
           .to_h { |row| [row[:path], row] }

    expect(rows['b.rb'][:score]).to be_within(0.0001).of(0.6722)
    expect(rows['b.rb'][:coverage]).to eq(0.5)
  end

  it 'uses maximum coverage risk for scoring but renders an em dash for a file absent from coverage data' do
    rows = scorer(fan_in: files.to_h { |file| [file, 0] },
                  coverage: { 'a.rb' => 1.0, 'c.rb' => 0.0, 'd.rb' => 0.25 },
                  weights: { fan_in: 0.0, fan_out: 0.0, complexity: 0.0, churn: 0.0, coverage: 1.0 }).call
           .to_h { |row| [row[:path], row] }

    expect(rows['b.rb'][:score]).to eq(rows['c.rb'][:score])
    expect(rows['b.rb'][:score]).to be > rows['d.rb'][:score]
    expect(rows['b.rb'][:coverage]).to eq('—')
    expect(rows['c.rb'][:coverage]).to eq(0.0)
  end

  it 'a file with higher fan_out outscores an otherwise-identical file' do
    files = %w[low.rb high.rb x.rb y.rb]
    rows = described_class.new(
      files: files,
      fan_in: { 'low.rb' => 1, 'high.rb' => 1, 'x.rb' => 0, 'y.rb' => 2 },
      fan_out: { 'low.rb' => 0, 'high.rb' => 5, 'x.rb' => 1, 'y.rb' => 2 },
      complexity: { 'low.rb' => 3, 'high.rb' => 3, 'x.rb' => 0, 'y.rb' => 1 },
      churn: { 'low.rb' => 4, 'high.rb' => 4, 'x.rb' => 0, 'y.rb' => 1 },
      weights: { fan_in: 0.25, fan_out: 0.10, complexity: 0.25, churn: 0.25, coverage: 0.0 }
    ).call.to_h { |row| [row[:path], row] }

    expect(rows['high.rb'][:score]).to be > rows['low.rb'][:score]
  end

  it 'uses rebalanced weights that sum to 1.0 when coverage is active' do
    weights = scorer(coverage: { 'a.rb' => 1.0, 'b.rb' => 0.5, 'c.rb' => 0.0, 'd.rb' => 0.25 },
                     weights: { fan_in: 0.20, fan_out: 0.10, complexity: 0.25, churn: 0.25,
                                coverage: 0.10, interaction: 0.10 }).normalized_weights

    expect(weights).to eq(fan_in: 0.20, fan_out: 0.10, complexity: 0.25, churn: 0.25, coverage: 0.10, interaction: 0.10)
    expect(weights.values.sum).to be_within(0.0001).of(1.0)
  end

  it 'percentile-ranks coverage risk so equivalent spreads contribute equivalent scores' do
    weights = { fan_in: 0.0, fan_out: 0.0, complexity: 0.0, churn: 0.0, coverage: 1.0 }
    no_fan_in = files.to_h { |file| [file, 0] }
    low_coverage_rows = scorer(fan_in: no_fan_in,
                               coverage: { 'a.rb' => 0.2, 'b.rb' => 0.8, 'c.rb' => 1.0, 'd.rb' => 1.0 },
                               weights: weights).call.to_h { |row| [row[:path], row] }
    high_coverage_rows = scorer(fan_in: no_fan_in,
                                coverage: { 'a.rb' => 0.85, 'b.rb' => 0.95, 'c.rb' => 1.0, 'd.rb' => 1.0 },
                                weights: weights).call.to_h { |row| [row[:path], row] }

    expect(low_coverage_rows['a.rb'][:score] - low_coverage_rows['b.rb'][:score])
      .to eq(high_coverage_rows['a.rb'][:score] - high_coverage_rows['b.rb'][:score])
    expect(low_coverage_rows['a.rb'][:score]).to eq(1.0)
    expect(high_coverage_rows['a.rb'][:score]).to eq(1.0)
  end

  it 'scores high fan-in and high coverage-risk above the pure additive coverage gap' do
    files = %w[low.rb b.rb f2.rb f3.rb f4.rb f5.rb f6.rb f7.rb f8.rb a.rb peer.rb]
    rows = described_class.new(
      files: files,
      fan_in: {
        'low.rb' => 0, 'b.rb' => 9, 'f2.rb' => 2, 'f3.rb' => 3, 'f4.rb' => 4, 'f5.rb' => 5,
        'f6.rb' => 6, 'f7.rb' => 7, 'f8.rb' => 8, 'a.rb' => 9, 'peer.rb' => 1
      },
      fan_out: files.to_h { |file| [file, 0] },
      complexity: files.to_h { |file| [file, 0] },
      churn: files.to_h { |file| [file, 0] },
      coverage: {
        'low.rb' => 1.0, 'b.rb' => 0.9, 'f2.rb' => 0.8, 'f3.rb' => 0.7, 'f4.rb' => 0.6,
        'f5.rb' => 0.5, 'f6.rb' => 0.4, 'f7.rb' => 0.3, 'f8.rb' => 0.2, 'a.rb' => 0.1,
        'peer.rb' => 0.1
      },
      weights: { fan_in: 0.20, fan_out: 0.10, complexity: 0.25, churn: 0.25, coverage: 0.10, interaction: 0.10 }
    ).call.to_h { |row| [row[:path], row] }

    additive_gap = 0.10 * (0.90 - 0.10)

    expect(rows['a.rb'][:fan_in_pct]).to eq(0.9)
    expect(rows['b.rb'][:fan_in_pct]).to eq(0.9)
    expect(rows['a.rb'][:score]).to be > rows['b.rb'][:score]
    expect(rows['a.rb'][:score] - rows['b.rb'][:score]).to be > additive_gap
  end

  it 'keeps the fan-in and coverage-risk interaction symmetric within tolerance' do
    files = (0..20).map { |index| "f#{index}.rb" }
    a_file = 'f19.rb'
    b_file = 'f18.rb'
    rows = described_class.new(
      files: files,
      fan_in: files.to_h do |file|
        value = if file == a_file
                  19
                elsif file == b_file
                  18
                else
                  file[/\d+/].to_i
                end
        [file, value]
      end,
      fan_out: files.to_h { |file| [file, 0] },
      complexity: files.to_h { |file| [file, 0] },
      churn: files.to_h { |file| [file, 0] },
      coverage: files.to_h do |file|
        risk = if file == a_file
                 0.90
               elsif file == b_file
                 0.95
               else
                 file[/\d+/].to_i / 20.0
               end
        [file, 1.0 - risk]
      end,
      weights: { fan_in: 0.20, fan_out: 0.10, complexity: 0.25, churn: 0.25, coverage: 0.10, interaction: 0.10 }
    ).call.to_h { |row| [row[:path], row] }

    expect(rows[a_file][:fan_in_pct]).to eq(0.95)
    expect(rows[a_file][:score]).to be > rows[b_file][:score]
    expect(rows[a_file][:score] - rows[b_file][:score]).to be < 0.02
  end

  it 'preserves score bounds with the interaction term' do
    files = %w[zero.rb max.rb]
    rows = described_class.new(
      files: files,
      fan_in: { 'zero.rb' => 0, 'max.rb' => 1 },
      fan_out: { 'zero.rb' => 0, 'max.rb' => 1 },
      complexity: { 'zero.rb' => 0, 'max.rb' => 1 },
      churn: { 'zero.rb' => 0, 'max.rb' => 1 },
      coverage: { 'zero.rb' => 1.0, 'max.rb' => 0.0 },
      weights: { fan_in: 0.20, fan_out: 0.10, complexity: 0.25, churn: 0.25, coverage: 0.10, interaction: 0.10 }
    ).call.to_h { |row| [row[:path], row] }

    expect(rows['max.rb'][:score]).to be <= 1.0
    expect(rows['max.rb'][:score]).to be_within(0.0001).of(1.0)
    expect(rows['zero.rb'][:score]).to eq(0.0)
  end

  it 'raises when branch threshold is not less than trunk threshold' do
    expect { scorer(branch_threshold: 85, trunk_threshold: 85) }
      .to raise_error(StudFinder::Scorer::ValidationError, /branch-threshold/)
  end

  it 'sorts rows by score descending' do
    scores = scorer.call.map { |row| row[:score] }

    expect(scores).to eq(scores.sort.reverse)
  end

  it 're-percentile-ranks the 50/50 commit-count and line-count churn composite' do
    rows = scorer(churn: { 'a.rb' => 0, 'b.rb' => 1, 'c.rb' => 10, 'd.rb' => 0 },
                  churn_lines: { 'a.rb' => 100, 'b.rb' => 0, 'c.rb' => 0, 'd.rb' => 0 }).call
           .to_h { |row| [row[:path], row] }

    expect(rows['a.rb'][:churn_commits]).to eq(0)
    expect(rows['a.rb'][:churn_lines]).to eq(100)
    expect(rows['a.rb'][:churn_pct]).to eq(0.6667)
    expect(rows['b.rb'][:churn_pct]).to eq(0.3333)
    expect(rows['c.rb'][:churn_pct]).to eq(0.6667)
  end

  it 'spreads uncorrelated churn composites across the full percentile range' do
    files = %w[min.rb count_low.rb mixed.rb count_high.rb max.rb]
    rows = described_class.new(
      files: files,
      fan_in: { 'min.rb' => 0, 'count_low.rb' => 1, 'mixed.rb' => 2, 'count_high.rb' => 3, 'max.rb' => 4 },
      fan_out: files.to_h { |file| [file, 0] },
      complexity: files.to_h { |file| [file, 0] },
      churn: { 'min.rb' => 0, 'count_low.rb' => 1, 'mixed.rb' => 2, 'count_high.rb' => 3, 'max.rb' => 4 },
      churn_lines: { 'min.rb' => 0, 'count_low.rb' => 4, 'mixed.rb' => 2, 'count_high.rb' => 1, 'max.rb' => 3 }
    ).call.to_h { |row| [row[:path], row] }

    churn_pcts = rows.values.map { |row| row[:churn_pct] }
    fan_in_pcts = rows.values.map { |row| row[:fan_in_pct] }

    expect(churn_pcts.min).to eq(0.0)
    expect(churn_pcts.max).to eq(1.0)
    expect(churn_pcts.max - churn_pcts.min).to eq(fan_in_pcts.max - fan_in_pcts.min)
  end

  it 'keeps identical churn count and line composites tied' do
    rows = scorer(churn: { 'a.rb' => 2, 'b.rb' => 2, 'c.rb' => 0, 'd.rb' => 4 },
                  churn_lines: { 'a.rb' => 10, 'b.rb' => 10, 'c.rb' => 0, 'd.rb' => 20 }).call
           .to_h { |row| [row[:path], row] }

    expect(rows['a.rb'][:churn_pct]).to eq(rows['b.rb'][:churn_pct])
  end

  it 'keeps float-equivalent churn composites tied after re-ranking' do
    files = %w[rank0.rb a.rb b.rb rank3.rb rank4.rb rank5.rb rank6.rb rank7.rb rank8.rb rank9.rb rank10.rb]
    rows = described_class.new(
      files: files,
      fan_in: files.to_h { |file| [file, 0] },
      fan_out: files.to_h { |file| [file, 0] },
      complexity: files.to_h { |file| [file, 0] },
      churn: {
        'b.rb' => 0, 'a.rb' => 1, 'rank0.rb' => 2, 'rank3.rb' => 3, 'rank4.rb' => 4, 'rank5.rb' => 5,
        'rank6.rb' => 6, 'rank7.rb' => 7, 'rank8.rb' => 8, 'rank9.rb' => 9, 'rank10.rb' => 10
      },
      churn_lines: {
        'rank0.rb' => 0, 'rank3.rb' => 1, 'a.rb' => 2, 'b.rb' => 3, 'rank4.rb' => 4, 'rank5.rb' => 5,
        'rank6.rb' => 6, 'rank7.rb' => 7, 'rank8.rb' => 8, 'rank9.rb' => 9, 'rank10.rb' => 10
      }
    ).call.to_h { |row| [row[:path], row] }

    expect(rows['a.rb'][:churn_pct]).to eq(rows['b.rb'][:churn_pct])
  end

  it 'returns zero churn percentiles when all churn counts and lines are identical' do
    rows = scorer(churn: files.to_h { |file| [file, 3] },
                  churn_lines: files.to_h { |file| [file, 9] }).call
           .to_h { |row| [row[:path], row] }

    expect(rows.values.map { |row| row[:churn_pct] }).to all(eq(0.0))
  end

  it 'warns when equal non-zero churn collapses to zero percentiles' do
    test_scorer = scorer(churn: files.to_h { |file| [file, 5] },
                         churn_lines: files.to_h { |file| [file, 10] })
    rows = test_scorer.call.to_h { |row| [row[:path], row] }

    expect(rows.values.map { |row| row[:churn_pct] }).to all(eq(0.0))
    expect(test_scorer.warnings).to include('insufficient_dispersion_churn')
  end

  it 'percentile-ranks per-file instability into instability_pct' do
    rows = scorer.call.to_h { |row| [row[:path], row] }

    # instability(fi, fo): a=0/3=0.0, b=1/3=0.3333, c=2/3=0.6667, d=0.0
    expect(rows['c.rb'][:instability]).to eq(0.6667)
    expect(rows['c.rb'][:instability_pct]).to eq(1.0)
    expect(rows['b.rb'][:instability_pct]).to eq(0.6667)
    expect(rows['a.rb'][:instability_pct]).to eq(0.0)
  end

  it 'emits coupling fields from the coupling hash' do
    coupling = {
      'a.rb' => { max_coupling: 0.8, max_coupling_partner: 'b.rb', partners: 3 },
      'b.rb' => { max_coupling: 0.2, max_coupling_partner: 'a.rb', partners: 1 }
    }
    rows = scorer(coupling: coupling).call.to_h { |row| [row[:path], row] }

    expect(rows['a.rb'][:max_coupling]).to eq(0.8)
    expect(rows['a.rb'][:max_coupling_partner]).to eq('b.rb')
    expect(rows['a.rb'][:coupling_partners]).to eq(3)
    expect(rows['a.rb'][:coupling_pct]).to eq(1.0)
    expect(rows['b.rb'][:max_coupling]).to eq(0.2)
    expect(rows['b.rb'][:max_coupling_partner]).to eq('a.rb')
    expect(rows['b.rb'][:coupling_partners]).to eq(1)
    # files absent from the hash are treated as 0.0 / 0 / empty partner
    expect(rows['c.rb'][:max_coupling]).to eq(0.0)
    expect(rows['c.rb'][:max_coupling_partner]).to eq('')
    expect(rows['c.rb'][:coupling_partners]).to eq(0)
  end

  it 'defaults max_coupling_partner to an empty string when a partner path is missing' do
    coupling = { 'a.rb' => { max_coupling: 0.5, partners: 2 } }
    rows = scorer(coupling: coupling).call.to_h { |row| [row[:path], row] }

    expect(rows['a.rb'][:max_coupling]).to eq(0.5)
    expect(rows['a.rb'][:max_coupling_partner]).to eq('')
  end

  it 'zeroes coupling fields when no coupling data is supplied' do
    rows = scorer.call.to_h { |row| [row[:path], row] }

    expect(rows['a.rb'][:max_coupling]).to eq(0.0)
    expect(rows['a.rb'][:max_coupling_partner]).to eq('')
    expect(rows['a.rb'][:coupling_partners]).to eq(0)
    expect(rows['a.rb'][:coupling_pct]).to eq(0.0)
  end
end

RSpec.describe StudFinder::Scorer, 'with coverage' do
  let(:files) { %w[a.rb b.rb] }
  let(:fan_in) { { 'a.rb' => 1, 'b.rb' => 0 } }
  let(:fan_out) { { 'a.rb' => 0, 'b.rb' => 1 } }
  let(:complexity) { { 'a.rb' => 0, 'b.rb' => 1 } }
  let(:churn) { { 'a.rb' => 0, 'b.rb' => 0 } }
  let(:coverage) { { 'a.rb' => 1.0, 'b.rb' => 0.0 } }

  it 'uses the Option B 5-factor formula without a global divisor' do
    scorer = described_class.new(
      files: files,
      fan_in: fan_in,
      fan_out: fan_out,
      complexity: complexity,
      churn: churn,
      coverage: coverage,
      weights: { fan_in: 0.20, fan_out: 0.10, complexity: 0.25, churn: 0.25, coverage: 0.10, interaction: 0.10 }
    )

    expect(scorer.normalized_weights).to eq(
      fan_in: 0.20, fan_out: 0.10, complexity: 0.25, churn: 0.25, coverage: 0.10, interaction: 0.10
    )
    rows = scorer.call.to_h { |row| [row[:path], row] }
    # b.rb: fan_out 1.0 (0.10) + complexity 1.0 (0.25) + uncovered coverage 1.0 (0.10).
    expect(rows['b.rb'][:score]).to eq(0.45)
    expect(rows['a.rb'][:coverage]).to eq(1.0)
  end
end

RSpec.describe StudFinder::Scorer, 'LOC instrumentation' do
  it 'emits LOC and language-scope percentile ranks without affecting score' do
    files = %w[small.rb medium.rb large.rb]
    base = {
      files: files,
      fan_in: files.to_h { |file| [file, 0] },
      fan_out: files.to_h { |file| [file, 0] },
      complexity: files.to_h { |file| [file, 0] },
      churn: files.to_h { |file| [file, 0] },
      weights: { fan_in: 1.0, fan_out: 0.0, complexity: 0.0, churn: 0.0, coverage: 0.0 }
    }

    without_loc = described_class.new(**base).call.to_h { |row| [row[:path], row] }
    with_loc = described_class.new(**base, loc: { 'small.rb' => 1, 'medium.rb' => 5, 'large.rb' => 10 })
                              .call.to_h { |row| [row[:path], row] }

    expect(with_loc['small.rb'][:loc]).to eq(1)
    expect(with_loc['small.rb'][:loc_pct]).to eq(0.0)
    expect(with_loc['medium.rb'][:loc_pct]).to eq(0.5)
    expect(with_loc['large.rb'][:loc_pct]).to eq(1.0)
    expect(with_loc.transform_values { |row| row[:score] })
      .to eq(without_loc.transform_values { |row| row[:score] })
  end
end
