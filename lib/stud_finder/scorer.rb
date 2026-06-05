# frozen_string_literal: true

require_relative 'normalizer'

module StudFinder
  class Scorer
    DEFAULT_WEIGHTS = { fan_in: 0.25, fan_out: 0.10, complexity: 0.25, churn: 0.25, coverage: 0.15 }.freeze
    RENORMALIZED_KEYS = %i[fan_in fan_out complexity churn].freeze

    class ValidationError < StandardError; end

    attr_reader :normalized_weights

    def initialize(files:, fan_in:, fan_out:, complexity:, churn:, churn_lines: nil, coverage: nil,
                   weights: DEFAULT_WEIGHTS, branch_threshold: 50, trunk_threshold: 85, coupling: nil)
      @files = files
      @fan_in = fan_in
      @fan_out = fan_out
      @complexity = complexity
      @churn = churn
      @churn_lines = churn_lines || churn
      @coverage = coverage
      @weights = weights
      @branch_threshold = branch_threshold
      @trunk_threshold = trunk_threshold
      @coupling = coupling
      validate!
      @normalized_weights = normalize_weights
    end

    def call
      pcts = {
        fan_in: Normalizer.percentile_rank(@fan_in, @files),
        fan_out: Normalizer.percentile_rank(@fan_out, @files),
        complexity: Normalizer.percentile_rank(@complexity, @files),
        churn: composite_churn_pct,
        instability: instability_pct,
        coupling: coupling_pct
      }

      rows = @files.each_with_index.map do |file, index|
        score = weighted_score(file, pcts)
        [index, result_row(file, score, pcts)]
      end

      rows.sort_by { |index, row| [-row[:score], index] }
          .map.with_index(1) do |(_index, row), rank|
        row.merge(rank: rank)
      end
    end

    private

    def validate!
      return if @branch_threshold < @trunk_threshold

      raise ValidationError, 'Error: branch-threshold must be strictly less than trunk-threshold.'
    end

    def normalize_weights
      active_total = RENORMALIZED_KEYS.sum { |key| @weights.fetch(key, 0.0) }
      if !coverage_available? && active_total <= 0.0
        raise ValidationError,
              'Error: active weights must be greater than 0.0.'
      end

      @active_weights = if active_total > 0.0
                          RENORMALIZED_KEYS.to_h { |key| [key, @weights.fetch(key, 0.0) / active_total] }
                                           .merge(coverage: nil)
                        else
                          RENORMALIZED_KEYS.to_h { |key| [key, 0.0] }.merge(coverage: nil)
                        end

      return @weights if coverage_available?

      @active_weights
    end

    def weighted_score(file, pcts)
      return active_weights_score(file, pcts) unless coverage_available?

      file_coverage = @coverage.fetch(file, 0.0)
      structural_score(@normalized_weights, file, pcts) +
        (@normalized_weights[:coverage] * (1.0 - file_coverage))
    end

    def active_weights_score(file, pcts)
      structural_score(@active_weights, file, pcts)
    end

    def structural_score(weights, file, pcts)
      (weights[:fan_in] * pcts[:fan_in].fetch(file)) +
        (weights[:fan_out] * pcts[:fan_out].fetch(file)) +
        (weights[:complexity] * pcts[:complexity].fetch(file)) +
        (weights[:churn] * pcts[:churn].fetch(file))
    end

    def composite_churn_pct
      count_pct = Normalizer.percentile_rank(@churn, @files)
      line_pct = Normalizer.percentile_rank(@churn_lines, @files)

      @files.to_h do |file|
        [file, (0.5 * count_pct.fetch(file)) + (0.5 * line_pct.fetch(file))]
      end
    end

    def result_row(file, score, pcts)
      fi = @fan_in.fetch(file, 0).to_i
      fo = @fan_out.fetch(file, 0).to_i
      {
        path: file,
        score: score.round(4),
        classification: classification(pcts[:fan_in].fetch(file)),
        fan_in: fi,
        fan_in_pct: pcts[:fan_in].fetch(file).round(4),
        fan_out: fo,
        fan_out_pct: pcts[:fan_out].fetch(file).round(4),
        instability: instability(fi, fo),
        instability_pct: pcts[:instability].fetch(file).round(4),
        complexity: @complexity.fetch(file, 0).to_i,
        complexity_pct: pcts[:complexity].fetch(file).round(4),
        churn_commits: @churn.fetch(file, 0).to_i,
        churn_lines: @churn_lines.fetch(file, 0).to_i,
        churn_pct: pcts[:churn].fetch(file).round(4),
        **coupling_fields(file, pcts),
        coverage: coverage_available? ? @coverage.fetch(file, 0.0).round(4) : nil
      }
    end

    def coupling_fields(file, pcts)
      partner = @coupling&.fetch(file, nil)
      {
        max_coupling: partner ? partner.fetch(:max_coupling, 0.0).to_f.round(4) : 0.0,
        coupling_partners: partner ? partner.fetch(:partners, 0).to_i : 0,
        coupling_pct: pcts[:coupling].fetch(file, 0.0).round(4)
      }
    end

    def instability(fan_in, fan_out)
      total = fan_in + fan_out
      return 0.0 if total.zero?

      (fan_out.to_f / total).round(4)
    end

    def instability_pct
      values = @files.to_h { |file| [file, instability(@fan_in.fetch(file, 0).to_i, @fan_out.fetch(file, 0).to_i)] }
      Normalizer.percentile_rank(values, @files)
    end

    def coupling_pct
      values = @files.to_h do |file|
        partner = @coupling&.fetch(file, nil)
        [file, partner ? partner.fetch(:max_coupling, 0.0).to_f : 0.0]
      end
      Normalizer.percentile_rank(values, @files)
    end

    def coverage_available?
      !@coverage.nil?
    end

    def classification(fan_in_pct)
      return 'trunk' if fan_in_pct >= @trunk_threshold / 100.0
      return 'branch' if fan_in_pct >= @branch_threshold / 100.0

      'leaf'
    end
  end
end
