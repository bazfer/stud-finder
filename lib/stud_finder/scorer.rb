# frozen_string_literal: true

require_relative 'normalizer'

module StudFinder
  class Scorer
    DEFAULT_WEIGHTS = { fan_in: 0.35, complexity: 0.25, churn: 0.25, coverage: 0.15 }.freeze

    class ValidationError < StandardError; end

    attr_reader :normalized_weights

    # rubocop:disable Metrics/ParameterLists
    def initialize(files:, fan_in:, fan_out:, complexity:, churn:, churn_lines: nil, coverage: nil,
                   weights: DEFAULT_WEIGHTS, branch_threshold: 50, trunk_threshold: 85)
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
      validate!
      @normalized_weights = normalize_weights
    end
    # rubocop:enable Metrics/ParameterLists

    def call
      fan_in_pct = Normalizer.percentile_rank(@fan_in, @files)
      complexity_pct = Normalizer.percentile_rank(@complexity, @files)
      churn_pct = composite_churn_pct

      rows = @files.each_with_index.map do |file, index|
        score = weighted_score(file, fan_in_pct, complexity_pct, churn_pct)
        [index, result_row(file, score, fan_in_pct, complexity_pct, churn_pct)]
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
      three_total = @weights.fetch(:fan_in, 0.0) + @weights.fetch(:complexity, 0.0) + @weights.fetch(:churn, 0.0)
      if !coverage_available? && three_total <= 0.0
        raise ValidationError,
              'Error: active weights must be greater than 0.0.'
      end

      @three_factor_weights = if three_total > 0.0
                                {
                                  fan_in: @weights.fetch(:fan_in, 0.0) / three_total,
                                  complexity: @weights.fetch(:complexity, 0.0) / three_total,
                                  churn: @weights.fetch(:churn, 0.0) / three_total,
                                  coverage: nil
                                }
                              else
                                { fan_in: 0.0, complexity: 0.0, churn: 0.0, coverage: nil }
                              end

      return @weights if coverage_available?

      @three_factor_weights
    end

    def weighted_score(file, fan_in_pct, complexity_pct, churn_pct)
      return three_factor_score(file, fan_in_pct, complexity_pct, churn_pct) unless coverage_available?

      file_coverage = @coverage.fetch(file, 0.0)
      (@normalized_weights[:fan_in] * fan_in_pct.fetch(file)) +
        (@normalized_weights[:complexity] * complexity_pct.fetch(file)) +
        (@normalized_weights[:churn] * churn_pct.fetch(file)) +
        (@normalized_weights[:coverage] * (1.0 - file_coverage))
    end

    def three_factor_score(file, fan_in_pct, complexity_pct, churn_pct)
      (@three_factor_weights[:fan_in] * fan_in_pct.fetch(file)) +
        (@three_factor_weights[:complexity] * complexity_pct.fetch(file)) +
        (@three_factor_weights[:churn] * churn_pct.fetch(file))
    end

    def composite_churn_pct
      count_pct = Normalizer.percentile_rank(@churn, @files)
      line_pct = Normalizer.percentile_rank(@churn_lines, @files)

      @files.to_h do |file|
        [file, (0.5 * count_pct.fetch(file)) + (0.5 * line_pct.fetch(file))]
      end
    end

    def result_row(file, score, fan_in_pct, complexity_pct, churn_pct)
      fi = @fan_in.fetch(file, 0).to_i
      fo = @fan_out.fetch(file, 0).to_i
      {
        path: file,
        score: score.round(4),
        classification: classification(fan_in_pct.fetch(file)),
        fan_in: fi,
        fan_in_pct: fan_in_pct.fetch(file).round(4),
        fan_out: fo,
        instability: instability(fi, fo),
        complexity: @complexity.fetch(file, 0).to_i,
        complexity_pct: complexity_pct.fetch(file).round(4),
        churn_commits: @churn.fetch(file, 0).to_i,
        churn_lines: @churn_lines.fetch(file, 0).to_i,
        churn_pct: churn_pct.fetch(file).round(4),
        coverage: coverage_available? ? @coverage.fetch(file, 0.0).round(4) : nil
      }
    end

    def instability(fan_in, fan_out)
      total = fan_in + fan_out
      return 0.0 if total.zero?

      (fan_out.to_f / total).round(4)
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
