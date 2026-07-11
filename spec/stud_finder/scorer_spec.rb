# frozen_string_literal: true

require 'spec_helper'
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
      weights: { fan_in: 0.25, fan_out: 0.10, complexity: 0.25, churn: 0.25, coverage: 0.0 },
      branch_threshold: 50,
      trunk_threshold: 85
    }.merge(overrides)

    described_class.new(**options)
  end

  it 'renormalizes active weights (four-factor) to sum to 1.0 without coverage' do
    weights = scorer.normalized_weights

    expect(weights[:fan_in]).to be_within(0.0001).of(0.2941)
    expect(weights[:fan_out]).to be_within(0.0001).of(0.1176)
    expect(weights[:complexity]).to be_within(0.0001).of(0.2941)
    expect(weights[:churn]).to be_within(0.0001).of(0.2941)
    expect(weights[:coverage]).to be_nil
    expect(weights.values.compact.sum).to be_within(0.0001).of(1.0)
  end

  it 'produces scores in the inclusive 0.0 to 1.0 range' do
    scores = scorer.call.map { |row| row[:score] }

    expect(scores).to all(be_between(0.0, 1.0).inclusive)
  end

  it 'keeps the four-factor path unchanged when coverage is unavailable' do
    rows = scorer.call.to_h { |row| [row[:path], row] }

    expect(rows['b.rb'][:score]).to eq(0.7647)
    expect(rows['b.rb'][:coverage]).to be_nil
  end

  it 'classifies by composite score' do
    rows = scorer.call.to_h { |row| [row[:path], row] }

    expect(rows['a.rb'][:classification]).to eq('leaf')
    expect(rows['b.rb'][:classification]).to eq('branch')
    expect(rows['c.rb'][:classification]).to eq('branch')
  end

  it 'classifies max complexity, max churn, zero coverage, and median fan_in as trunk' do
    files = %w[low.rb risky.rb fan_in_top.rb]
    rows = described_class.new(
      files: files,
      fan_in: { 'low.rb' => 0, 'risky.rb' => 1, 'fan_in_top.rb' => 2 },
      fan_out: { 'low.rb' => 0, 'risky.rb' => 5, 'fan_in_top.rb' => 1 },
      complexity: { 'low.rb' => 0, 'risky.rb' => 10, 'fan_in_top.rb' => 1 },
      churn: { 'low.rb' => 0, 'risky.rb' => 10, 'fan_in_top.rb' => 1 },
      coverage: { 'low.rb' => 1.0, 'risky.rb' => 0.0, 'fan_in_top.rb' => 1.0 },
      weights: { fan_in: 0.25, fan_out: 0.10, complexity: 0.25, churn: 0.25, coverage: 0.15 }
    ).call.to_h { |row| [row[:path], row] }

    expect(rows['risky.rb'][:fan_in_pct]).to eq(0.5)
    expect(rows['risky.rb'][:score]).to eq(0.875)
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
      weights: { fan_in: 0.25, fan_out: 0.10, complexity: 0.25, churn: 0.25, coverage: 0.15 },
      branch_threshold: 30,
      trunk_threshold: 65
    ).call.to_h { |row| [row[:path], row] }

    expect(rows['fan_in_top.rb'][:fan_in_pct]).to eq(1.0)
    expect(rows['fan_in_top.rb'][:complexity_pct]).to eq(0.5)
    expect(rows['fan_in_top.rb'][:churn_pct]).to eq(0.5)
    expect(rows['fan_in_top.rb'][:score]).to eq(0.675)
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
      weights: { fan_in: 0.25, fan_out: 0.10, complexity: 0.25, churn: 0.25, coverage: 0.15 },
      branch_threshold: 30,
      trunk_threshold: 90
    ).call.to_h { |row| [row[:path], row] }

    expect(rows['trunk.rb'][:score]).to eq(1.0)
    expect(rows['trunk.rb'][:classification]).to eq('trunk')
    expect(rows['near_trunk.rb'][:score]).to eq(0.6667)
    expect(rows['near_trunk.rb'][:classification]).to eq('branch')
    expect(rows['leaf.rb'][:classification]).to eq('leaf')
  end

  it 'uses five-factor scoring when coverage is provided' do
    rows = scorer(coverage: { 'a.rb' => 1.0, 'b.rb' => 0.5, 'c.rb' => 0.0, 'd.rb' => 0.25 },
                  weights: { fan_in: 0.25, fan_out: 0.10, complexity: 0.25, churn: 0.25, coverage: 0.15 }).call
           .to_h { |row| [row[:path], row] }

    expect(rows['b.rb'][:score]).to be_within(0.0001).of(0.7)
    expect(rows['b.rb'][:coverage]).to eq(0.5)
  end

  it 'uses maximum coverage risk for scoring but renders an em dash for a file absent from coverage data' do
    rows = scorer(coverage: { 'a.rb' => 1.0, 'c.rb' => 0.0, 'd.rb' => 0.25 },
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

  it 'does not renormalize weights when coverage is active' do
    weights = scorer(coverage: { 'a.rb' => 1.0, 'b.rb' => 0.5, 'c.rb' => 0.0, 'd.rb' => 0.25 },
                     weights: { fan_in: 0.25, fan_out: 0.10, complexity: 0.25, churn: 0.25,
                                coverage: 0.15 }).normalized_weights

    expect(weights).to eq(fan_in: 0.25, fan_out: 0.10, complexity: 0.25, churn: 0.25, coverage: 0.15)
    expect(weights.values.sum).to be_within(0.0001).of(1.0)
  end

  it 'percentile-ranks coverage risk so equivalent spreads contribute equivalent scores' do
    weights = { fan_in: 0.0, fan_out: 0.0, complexity: 0.0, churn: 0.0, coverage: 1.0 }
    low_coverage_rows = scorer(coverage: { 'a.rb' => 0.2, 'b.rb' => 0.8, 'c.rb' => 1.0, 'd.rb' => 1.0 },
                               weights: weights).call.to_h { |row| [row[:path], row] }
    high_coverage_rows = scorer(coverage: { 'a.rb' => 0.85, 'b.rb' => 0.95, 'c.rb' => 1.0, 'd.rb' => 1.0 },
                                weights: weights).call.to_h { |row| [row[:path], row] }

    expect(low_coverage_rows['a.rb'][:score] - low_coverage_rows['b.rb'][:score])
      .to eq(high_coverage_rows['a.rb'][:score] - high_coverage_rows['b.rb'][:score])
    expect(low_coverage_rows['a.rb'][:score]).to eq(1.0)
    expect(high_coverage_rows['a.rb'][:score]).to eq(1.0)
  end

  it 'raises when branch threshold is not less than trunk threshold' do
    expect { scorer(branch_threshold: 85, trunk_threshold: 85) }
      .to raise_error(StudFinder::Scorer::ValidationError, /branch-threshold/)
  end

  it 'sorts rows by score descending' do
    scores = scorer.call.map { |row| row[:score] }

    expect(scores).to eq(scores.sort.reverse)
  end

  it 'uses a 50/50 composite of commit-count and line-count churn percentiles' do
    rows = scorer(churn: { 'a.rb' => 0, 'b.rb' => 1, 'c.rb' => 10, 'd.rb' => 0 },
                  churn_lines: { 'a.rb' => 100, 'b.rb' => 0, 'c.rb' => 0, 'd.rb' => 0 }).call
           .to_h { |row| [row[:path], row] }

    expect(rows['a.rb'][:churn_commits]).to eq(0)
    expect(rows['a.rb'][:churn_lines]).to eq(100)
    expect(rows['a.rb'][:churn_pct]).to eq(0.5)
    expect(rows['b.rb'][:churn_pct]).to eq(0.3333)
    expect(rows['c.rb'][:churn_pct]).to eq(0.5)
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

  it 'uses the 5-factor formula without renormalizing weights' do
    scorer = described_class.new(
      files: files,
      fan_in: fan_in,
      fan_out: fan_out,
      complexity: complexity,
      churn: churn,
      coverage: coverage,
      weights: { fan_in: 0.25, fan_out: 0.10, complexity: 0.25, churn: 0.25, coverage: 0.15 }
    )

    expect(scorer.normalized_weights).to eq(fan_in: 0.25, fan_out: 0.10, complexity: 0.25, churn: 0.25, coverage: 0.15)
    rows = scorer.call.to_h { |row| [row[:path], row] }
    # b.rb: fan_out 1.0 (0.10) + complexity 1.0 (0.25) + uncovered coverage 1.0 (0.15) = 0.5
    expect(rows['b.rb'][:score]).to eq(0.5)
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
