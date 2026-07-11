# frozen_string_literal: true

require_relative 'dispersion_warnings'
require_relative 'normalizer'

module StudFinder
  # rubocop:disable Metrics/ClassLength
  class Scorer
    DEFAULT_WEIGHTS = {
      fan_in: 0.19, fan_out: 0.095, complexity: 0.2375, churn: 0.2375, coverage: 0.095,
      interaction: 0.095, coupling: 0.05
    }.freeze
    BASE_WEIGHT_KEYS = %i[fan_in fan_out complexity churn].freeze
    WEIGHT_KEYS = %i[fan_in fan_out complexity churn coverage interaction coupling].freeze
    COMPLEXITY_FLOOR = 15
    FAN_IN_FLOOR = 25

    class ValidationError < StandardError; end

    attr_reader :normalized_weights, :warnings

    def initialize(files:, fan_in:, fan_out:, complexity:, churn:, churn_lines: nil, loc: nil, loc_pct: nil,
                   coverage: nil, weights: DEFAULT_WEIGHTS, branch_threshold: 50, trunk_threshold: 85, coupling: nil)
      @files = files
      @fan_in = fan_in
      @fan_out = fan_out
      @complexity = complexity
      @churn = churn
      @churn_lines = churn_lines || churn
      @loc = loc || @files.to_h { |file| [file, 0] }
      @loc_pct = loc_pct
      @coverage = coverage
      @weights = weights
      @branch_threshold = branch_threshold
      @trunk_threshold = trunk_threshold
      @coupling = coupling
      @warnings = []
      validate!
      @normalized_weights = normalize_weights
    end

    def call
      pcts = {
        fan_in: Normalizer.percentile_rank(@fan_in, @files),
        fan_out: Normalizer.percentile_rank(@fan_out, @files),
        complexity: Normalizer.percentile_rank(@complexity, @files),
        churn: composite_churn_pct,
        loc: @loc_pct || Normalizer.percentile_rank(@loc, @files),
        instability: instability_pct,
        coupling: coupling_pct,
        coverage: coverage_risk_pct
      }
      @warnings = insufficient_dispersion_warnings(pcts)

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
      active_keys = BASE_WEIGHT_KEYS.dup
      active_keys += %i[coverage interaction] if coverage_available?
      active_keys << :coupling if coupling_available?

      active_total = active_keys.sum { |key| @weights.fetch(key, 0.0) }
      if active_total <= 0.0
        raise ValidationError,
              'Error: active weights must be greater than 0.0.'
      end

      WEIGHT_KEYS.to_h do |key|
        [key, active_keys.include?(key) ? @weights.fetch(key, 0.0) / active_total : nil]
      end
    end

    def weighted_score(file, pcts)
      score = structural_score(@normalized_weights, file, pcts)
      score += @normalized_weights[:coverage] * pcts[:coverage].fetch(file) if coverage_available?
      score += interaction_score(file, pcts) if coverage_available?
      score += @normalized_weights[:coupling] * pcts[:coupling].fetch(file) if coupling_available?

      score.clamp(0.0, 1.0)
    end

    def interaction_score(file, pcts)
      @normalized_weights.fetch(:interaction, 0.0) * pcts[:fan_in].fetch(file) * pcts[:coverage].fetch(file)
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
      @churn_signal_raw = @files.to_h do |file|
        [file, ((0.5 * count_pct.fetch(file)) + (0.5 * line_pct.fetch(file))).round(10)]
      end

      Normalizer.percentile_rank(@churn_signal_raw, @files)
    end

    def churn_dispersion_raw_source
      @files.to_h do |file|
        [file, @churn.fetch(file, 0).to_f.abs + @churn_lines.fetch(file, 0).to_f.abs]
      end
    end

    def result_row(file, score, pcts)
      fi = @fan_in.fetch(file, 0).to_i
      fo = @fan_out.fetch(file, 0).to_i
      rounded_score = score.round(4)
      complexity = @complexity.fetch(file, 0).to_i
      floored_class, floor_escalation = floored_classification(classification(rounded_score), complexity, fi)
      {
        path: file,
        score: rounded_score,
        classification: floored_class,
        escalation: floor_escalation,
        fan_in: fi,
        fan_in_pct: pcts[:fan_in].fetch(file).round(4),
        fan_out: fo,
        fan_out_pct: pcts[:fan_out].fetch(file).round(4),
        instability: instability(fi, fo),
        instability_pct: pcts[:instability].fetch(file).round(4),
        complexity: complexity,
        complexity_pct: pcts[:complexity].fetch(file).round(4),
        churn_commits: @churn.fetch(file, 0).to_i,
        churn_lines: @churn_lines.fetch(file, 0).to_i,
        churn_pct: pcts[:churn].fetch(file).round(4),
        loc: @loc.fetch(file, 0).to_i,
        loc_pct: pcts[:loc].fetch(file).round(4),
        **coupling_fields(file, pcts),
        coverage: coverage_value(file)
      }
    end

    def coupling_fields(file, pcts)
      partner = @coupling&.fetch(file, nil)
      {
        max_coupling: partner ? partner.fetch(:max_coupling, 0.0).to_f.round(4) : 0.0,
        max_coupling_partner: partner ? partner.fetch(:max_coupling_partner, nil).to_s : '',
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
      Normalizer.percentile_rank(coupling_values, @files)
    end

    def coupling_values
      @files.to_h do |file|
        partner = @coupling&.fetch(file, nil)
        [file, partner ? partner.fetch(:max_coupling, 0.0).to_f : 0.0]
      end
    end

    def coverage_risk_pct
      return {} unless coverage_available?

      Normalizer.percentile_rank(coverage_risk_values, @files)
    end

    def coverage_risk_values
      return {} unless coverage_available?

      @files.to_h { |file| [file, 1.0 - @coverage.fetch(file, 0.0)] }
    end

    def interaction_values(pcts)
      return {} unless coverage_available?

      @files.to_h { |file| [file, pcts[:fan_in].fetch(file) * pcts[:coverage].fetch(file)] }
    end

    def insufficient_dispersion_warnings(pcts)
      raw_sources = {
        fan_in: @fan_in,
        fan_out: @fan_out,
        complexity: @complexity,
        churn: churn_dispersion_raw_source
      }
      if coverage_available?
        raw_sources[:coverage] = coverage_risk_values
        raw_sources[:interaction] = interaction_values(pcts)
        pcts = pcts.merge(interaction: Normalizer.percentile_rank(raw_sources[:interaction], @files))
      end
      raw_sources[:coupling] = coupling_values if coupling_available?

      DispersionWarnings.build(files: @files, pcts: pcts, raw_sources: raw_sources)
    end

    def coverage_value(file)
      return nil unless coverage_available?
      return '—' unless @coverage.key?(file)

      @coverage.fetch(file).round(4)
    end

    def coverage_available?
      !@coverage.nil?
    end

    def coupling_available?
      return false if @coupling.nil? || @coupling.empty?

      @files.any? do |file|
        partner = @coupling.fetch(file, nil)
        partner.respond_to?(:fetch) && partner.fetch(:max_coupling, 0.0).to_f.positive?
      rescue ArgumentError, TypeError
        false
      end
    end

    def classification(score)
      return 'trunk' if score >= @trunk_threshold / 100.0
      return 'branch' if score >= @branch_threshold / 100.0

      'leaf'
    end

    def floored_classification(classification, complexity, fan_in)
      return [classification, ''] unless classification == 'leaf'

      if complexity >= COMPLEXITY_FLOOR
        ['branch', 'complexity_floor']
      elsif fan_in >= FAN_IN_FLOOR
        ['branch', 'fan_in_floor']
      else
        [classification, '']
      end
    end
  end
  # rubocop:enable Metrics/ClassLength
end
