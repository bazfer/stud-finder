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
      weights: { fan_in: 0.35, complexity: 0.25, churn: 0.25, coverage: 0.0 },
      branch_threshold: 50,
      trunk_threshold: 85
    }.merge(overrides)

    described_class.new(**options)
  end

  it 'renormalizes Phase 1 active weights to sum to 1.0' do
    weights = scorer.normalized_weights

    expect(weights[:fan_in]).to be_within(0.0001).of(0.4118)
    expect(weights[:complexity]).to be_within(0.0001).of(0.2941)
    expect(weights[:churn]).to be_within(0.0001).of(0.2941)
    expect(weights[:coverage]).to be_nil
    expect(weights.values.compact.sum).to be_within(0.0001).of(1.0)
  end

  it 'produces scores in the inclusive 0.0 to 1.0 range' do
    scores = scorer.call.map { |row| row[:score] }

    expect(scores).to all(be_between(0.0, 1.0).inclusive)
  end

  it 'classifies by fan_in percentile only' do
    rows = scorer.call.to_h { |row| [row[:path], row] }

    expect(rows['a.rb'][:classification]).to eq('trunk')
    expect(rows['b.rb'][:classification]).to eq('branch')
    expect(rows['c.rb'][:classification]).to eq('leaf')
  end

  it 'uses four-factor scoring when coverage is provided' do
    rows = scorer(coverage: { 'a.rb' => 1.0, 'b.rb' => 0.5, 'c.rb' => 0.0, 'd.rb' => 0.25 },
                  weights: { fan_in: 0.35, complexity: 0.25, churn: 0.25, coverage: 0.15 }).call
           .to_h { |row| [row[:path], row] }

    expect(rows['b.rb'][:score]).to be_within(0.0001).of(0.725)
    expect(rows['b.rb'][:coverage]).to eq(0.5)
  end

  it 'uses four-factor scoring with uncovered coverage for a file absent from coverage data' do
    rows = scorer(coverage: { 'a.rb' => 1.0, 'c.rb' => 0.0, 'd.rb' => 0.25 },
                  weights: { fan_in: 0.35, complexity: 0.25, churn: 0.25, coverage: 0.15 }).call
           .to_h { |row| [row[:path], row] }

    expect(rows['b.rb'][:score]).to be_within(0.0001).of(0.8)
    expect(rows['b.rb'][:coverage]).to eq(0.0)
  end

  it 'does not renormalize weights when coverage is active' do
    weights = scorer(coverage: { 'a.rb' => 1.0, 'b.rb' => 0.5, 'c.rb' => 0.0, 'd.rb' => 0.25 },
                     weights: { fan_in: 0.35, complexity: 0.25, churn: 0.25, coverage: 0.15 }).normalized_weights

    expect(weights).to eq(fan_in: 0.35, complexity: 0.25, churn: 0.25, coverage: 0.15)
    expect(weights.values.sum).to be_within(0.0001).of(1.0)
  end

  it 'uses 1.0 minus coverage fraction directly instead of percentile ranking coverage' do
    scorer_with_coverage = scorer(coverage: { 'a.rb' => 0.0, 'c.rb' => 1.0, 'd.rb' => 1.0 },
                                  weights: { fan_in: 0.0, complexity: 0.0, churn: 0.0, coverage: 1.0 })

    expect(scorer_with_coverage.normalized_weights).to eq(fan_in: 0.0, complexity: 0.0, churn: 0.0, coverage: 1.0)
    rows = scorer_with_coverage.call.to_h { |row| [row[:path], row] }

    expect(rows['a.rb'][:score]).to eq(1.0)
    expect(rows['b.rb'][:score]).to eq(1.0)
    expect(rows['d.rb'][:score]).to eq(0.0)
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
end

RSpec.describe StudFinder::Scorer, 'with coverage' do
  let(:files) { %w[a.rb b.rb] }
  let(:fan_in) { { 'a.rb' => 1, 'b.rb' => 0 } }
  let(:fan_out) { { 'a.rb' => 0, 'b.rb' => 1 } }
  let(:complexity) { { 'a.rb' => 0, 'b.rb' => 1 } }
  let(:churn) { { 'a.rb' => 0, 'b.rb' => 0 } }
  let(:coverage) { { 'a.rb' => 1.0, 'b.rb' => 0.0 } }

  it 'uses the 4-factor formula without renormalizing weights' do
    scorer = described_class.new(
      files: files,
      fan_in: fan_in,
      fan_out: fan_out,
      complexity: complexity,
      churn: churn,
      coverage: coverage,
      weights: { fan_in: 0.35, complexity: 0.25, churn: 0.25, coverage: 0.15 }
    )

    expect(scorer.normalized_weights).to eq(fan_in: 0.35, complexity: 0.25, churn: 0.25, coverage: 0.15)
    rows = scorer.call.to_h { |row| [row[:path], row] }
    expect(rows['b.rb'][:score]).to eq(0.4) # complexity 1.0 plus uncovered coverage term 1.0
    expect(rows['a.rb'][:coverage]).to eq(1.0)
  end
end
