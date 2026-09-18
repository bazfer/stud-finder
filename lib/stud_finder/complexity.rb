# frozen_string_literal: true

require 'json'
require 'open3'
require 'tempfile'
require_relative 'fact_cache'

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

    def initialize(repo_path:, files:, stderr: $stderr, cache_dir: nil)
      @repo_path = File.expand_path(repo_path)
      @files = files
      @stderr = stderr
      @cache_dir = cache_dir
    end

    attr_reader :cache_stats

    def call
      return cached_call if @cache_dir

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

    # A subprocess discovers Ruby's target from its repository cwd. The host
    # process may analyze several roots and must never change its global cwd.
    # This config object supplies the discovery base without reading the app's
    # RuboCop configuration (the subprocess also uses an explicit temp config).
    def self.target_ruby_version(repo_path)
      require 'rubocop'
      context = RuboCop::Config.new({ 'AllCops' => {} }, File.join(repo_path, '.rubocop-stud-finder-context.yml'))
      context.target_ruby_version
    end

    private

    def cached_call
      require 'rubocop'
      @target_ruby_version = self.class.target_ruby_version(@repo_path)
      cache = FactCache.new(directory: @cache_dir, namespace: cache_namespace, stderr: @stderr)
      fingerprints = @files.to_h { |file| [file, Digest::SHA256.file(File.join(@repo_path, file)).hexdigest] }
      counts = {}
      skipped = []
      missing = []
      @files.each do |file|
        value = cache.fetch(file, fingerprints[file])
        if value.is_a?(Integer) && value >= 0
          counts[file] = value
        else
          missing << file
        end
      end
      @cache_stats = { hits: @files.length - missing.length, misses: missing.length }
      unless missing.empty?
        result = run_embedded(missing)
        counts.merge!(result.counts)
        skipped = result.skipped_files
        result.counts.each { |file, count| cache.store(file, fingerprints[file], count) }
      end
      cache.retain(@files)
      cache.save
      Result.new(counts: counts, skipped_files: skipped)
    end

    def cache_namespace
      ['complexity-v2', RUBY_VERSION, RUBY_PLATFORM, RUBOCOP_CONFIG, @target_ruby_version,
       Gem.loaded_specs.values.map { |spec| [spec.name, spec.version.to_s] }.sort,
       Digest::SHA256.file(__FILE__).hexdigest].join(':')
    end

    # Use the same RuboCop runner/configuration/JSON formatter as the command,
    # without starting another Ruby process for a one-file edit. The uncached
    # subprocess implementation remains available as the comparison oracle.
    def run_embedded(files)
      Tempfile.create(['stud-finder-rubocop', '.yml']) do |config|
        config.write(RUBOCOP_CONFIG.sub('  DisabledByDefault:',
                                        "  TargetRubyVersion: #{@target_ruby_version}\n  DisabledByDefault:"))
        config.flush
        Tempfile.create(['stud-finder-complexity', '.json']) do |output|
          store = RuboCop::ConfigStore.new
          store.options_config = config.path
          options = { cache: false, parallel: false, formatters: [['json', output.path]], raise_cop_error: true }
          runner = RuboCop::Runner.new(options, store)
          runner.run(files.map { |file| File.join(@repo_path, file) })
          raise Error, 'Error: embedded RuboCop failed.' unless runner.errors.empty?

          parse(File.read(output.path), files)
        end
      end
    end

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
      physical_root = File.realpath(@repo_path) if File.exist?(@repo_path)
      return absolute.delete_prefix("#{physical_root}/") if physical_root && absolute.start_with?("#{physical_root}/")

      absolute.start_with?("#{@repo_path}/") ? absolute.delete_prefix("#{@repo_path}/") : path
    end

    def fatal_message(stderr)
      message = stderr.to_s.strip
      return 'Error: rubocop failed.' if message.empty?

      "Error: rubocop failed: #{message}"
    end
  end
end
