# frozen_string_literal: true

require 'json'

module StudFinder
  class Gate
    CHECKS = %w[trunk_touched low_evidence_high_score newness_trunk_adjacent].freeze
    HIGH_SCORE_THRESHOLD = 0.75
    LOW_EVIDENCE_THRESHOLD = 0.50

    Finding = Struct.new(:path, :language, :score, :evidence, :classification, :reason, keyword_init: true)
    Result = Struct.new(:checks, keyword_init: true) do
      def finding_count
        checks.values.sum(&:length)
      end

      def findings?
        finding_count.positive?
      end
    end

    class Error < StandardError; end

    def self.call(json)
      new(json).call
    end

    def initialize(json)
      @payload = JSON.parse(json)
    rescue JSON::ParserError => e
      raise Error, "Error: invalid JSON input: #{e.message}"
    end

    def call
      rows = Array(@payload.fetch('ruby', [])) + Array(@payload.fetch('javascript', []))
      Result.new(
        checks: {
          'trunk_touched' => trunk_touched(rows),
          'low_evidence_high_score' => low_evidence_high_score(rows),
          'newness_trunk_adjacent' => newness_trunk_adjacent(rows)
        }
      )
    end

    def self.markdown(result, enforce: false)
      (markdown_header(result, enforce: enforce) + markdown_summary(result) + markdown_details(result)).join("\n")
    end

    def self.markdown_header(result, enforce:)
      [
        '## Stud Finder gate',
        '',
        "**Mode:** #{enforce ? 'enforce' : 'observation'}",
        "**Summary:** #{pluralize(result.finding_count, 'finding')} across #{CHECKS.length} checks.",
        ''
      ]
    end
    private_class_method :markdown_header

    def self.markdown_summary(result)
      CHECKS.map do |check|
        findings = result.checks.fetch(check)
        status = findings.empty? ? '✅' : '⚠️'
        "- #{status} `#{check}` — #{pluralize(findings.length, 'finding')}"
      end + ['']
    end
    private_class_method :markdown_summary

    def self.markdown_details(result)
      CHECKS.flat_map { |check| markdown_check_detail(check, result.checks.fetch(check)) }
    end
    private_class_method :markdown_details

    def self.markdown_check_detail(check, findings)
      [
        "<details#{' open' if findings.any?}>",
        "<summary><strong>#{check}</strong> — #{pluralize(findings.length, 'finding')}</summary>",
        '',
        *markdown_finding_rows(findings),
        '',
        '</details>',
        ''
      ]
    end
    private_class_method :markdown_check_detail

    def self.markdown_finding_rows(findings)
      return ['_No findings._'] if findings.empty?

      ['| file | class | score | evidence | reason |', '| --- | --- | ---: | ---: | --- |'] +
        findings.map { |finding| markdown_finding_row(finding) }
    end
    private_class_method :markdown_finding_rows

    def self.markdown_finding_row(finding)
      "| `#{escape_md(finding.path)}` | #{escape_md(finding.classification)} | " \
        "#{format_number(finding.score)} | #{format_evidence(finding.evidence)} | #{escape_md(finding.reason)} |"
    end
    private_class_method :markdown_finding_row

    def self.pluralize(count, noun)
      "#{count} #{noun}#{'s' unless count == 1}"
    end
    private_class_method :pluralize

    def self.escape_md(value)
      value.to_s.gsub('|', '\\|')
    end
    private_class_method :escape_md

    def self.format_number(value)
      value.nil? ? '—' : format('%.4f', value.to_f)
    end
    private_class_method :format_number

    def self.format_evidence(value)
      value.nil? ? 'nil' : format_number(value)
    end
    private_class_method :format_evidence

    private

    def trunk_touched(rows)
      rows.select { |row| row['class'] == 'trunk' }.map do |row|
        finding(row, reason: 'Changed file is classified as trunk.')
      end
    end

    def low_evidence_high_score(rows)
      risky_rows = rows.select do |row|
        row.fetch('score', 0).to_f >= HIGH_SCORE_THRESHOLD && low_evidence?(row['evidence'])
      end
      risky_rows.map do |row|
        finding(row, reason: low_evidence_high_score_reason)
      end
    end

    def low_evidence_high_score_reason
      "Score is >= #{HIGH_SCORE_THRESHOLD} while evidence is nil or < #{LOW_EVIDENCE_THRESHOLD}."
    end

    def newness_trunk_adjacent(rows)
      rows.select { |row| row['new_file'] && row['escalation'] == 'trunk_adjacent' }.map do |row|
        finding(row, reason: 'New file is trunk-adjacent.')
      end
    end

    def low_evidence?(value)
      value.nil? || value.to_f < LOW_EVIDENCE_THRESHOLD
    end

    def finding(row, reason:)
      Finding.new(
        path: row.fetch('path'),
        language: row['language'],
        score: row['score'],
        evidence: row['evidence'],
        classification: row['class'],
        reason: reason
      )
    end
  end
end
