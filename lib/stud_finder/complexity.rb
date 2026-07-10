# frozen_string_literal: true

require 'json'
require 'open3'
require 'tempfile'

module StudFinder
  class Complexity
    Result = Struct.new(:counts, :skipped_files, keyword_init: true)

    class Error < StandardError; end

    COMPLEXITY_COP = 'Metrics/CyclomaticComplexity'
    COMPLEXITY_PATTERN = %r{\[(\d+)/0\]}
    INVALID_ENCODING_PATTERN = /invalid byte sequence/i
    PARSE_ERROR_COPS = %w[Lint/Syntax].freeze
    BATCH_SIZE = 500
    RUBOCOP_CONFIG = <<~YAML
      AllCops:
        DisabledByDefault: true
        NewCops: disable
      Metrics/CyclomaticComplexity:
        Enabled: true
        Max: 0
    YAML

    def initialize(repo_path:, files:, stderr: $stderr)
      @repo_path = File.expand_path(repo_path)
      @files = files
      @stderr = stderr
    end

    def call
      counts = zero_counts
      skipped = []

      @files.each_slice(BATCH_SIZE) do |batch|
        result = run_batch(batch)
        counts.merge!(result.counts) { |_file, old, new| [old, new].max }
        skipped.concat(result.skipped_files)
      end

      skipped.uniq.each { |file| counts.delete(file) }
      Result.new(counts: counts, skipped_files: skipped.uniq)
    rescue Errno::ENOENT
      raise Error, 'Error: rubocop not found. Install it: gem install rubocop'
    rescue JSON::ParserError => e
      raise Error, "Error: failed to parse RuboCop JSON output: #{e.message}"
    end

    private

    def run_batch(batch)
      stdout, stderr, status = run_rubocop(batch)
      raise Error, fatal_message(stderr) if status.exitstatus == 2
      raise Error, fatal_message(stderr) unless [0, 1].include?(status.exitstatus)

      parse(stdout, batch)
    end

    def run_rubocop(batch)
      Tempfile.create(['stud-finder-rubocop', '.yml']) do |config|
        config.write(RUBOCOP_CONFIG)
        config.close

        # Supplying an explicit temp config prevents RuboCop from loading the
        # target repo's .rubocop.yml while preserving our Max: 0 complexity rule.
        # RuboCop 1.88 has --force-default-config, but that option also ignores
        # explicit --config settings, so it cannot be combined with this custom
        # analysis config.
        Open3.capture3(
          'rubocop',
          '--config', config.path,
          '--format', 'json',
          '--',
          *batch,
          chdir: @repo_path
        )
      end
    end

    def parse(stdout, batch)
      payload = JSON.parse(stdout)
      counts = batch.to_h { |file| [file, 0] }
      skipped = []
      file_set = counts.keys.to_h { |file| [file, true] }

      Array(payload['files']).each do |entry|
        relative = normalize_path(entry['path'].to_s)
        next unless file_set[relative]

        offenses = Array(entry['offenses'])
        if invalid_encoding_error?(offenses)
          counts[relative] = 0
          next
        end

        if parse_error?(offenses)
          skipped << relative
          counts.delete(relative)
          @stderr.puts "Warning: skipping #{relative}; RuboCop could not parse file."
          next
        end

        counts[relative] = offenses.map { |offense| complexity_score(offense) }.max || 0
      end

      Result.new(counts: counts, skipped_files: skipped)
    end

    def zero_counts
      @files.to_h { |file| [file, 0] }
    end

    def complexity_score(offense)
      return 0 unless offense['cop_name'] == COMPLEXITY_COP

      offense.fetch('message', '').match(COMPLEXITY_PATTERN)&.[](1).to_i
    end

    def parse_error?(offenses)
      offenses.any? { |offense| PARSE_ERROR_COPS.include?(offense['cop_name']) || offense['fatal'] == true }
    end

    def invalid_encoding_error?(offenses)
      offenses.any? do |offense|
        parse_error?([offense]) && offense.fetch('message', '').match?(INVALID_ENCODING_PATTERN)
      end
    end

    def normalize_path(path)
      absolute = File.expand_path(path, @repo_path)
      absolute.start_with?("#{@repo_path}/") ? absolute.delete_prefix("#{@repo_path}/") : path
    end

    def fatal_message(stderr)
      message = stderr.to_s.strip
      return 'Error: rubocop failed.' if message.empty?

      "Error: rubocop failed: #{message}"
    end
  end
end
