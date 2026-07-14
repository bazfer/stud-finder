# frozen_string_literal: true

require 'csv'
require 'json'
require 'open3'
require 'optparse'
require 'pathname'
require 'set'
require 'time'
require_relative 'churn'
require_relative 'temporal_coupling'
require_relative 'warnings'
require_relative 'complexity'
require_relative 'diff'
require_relative 'coverage/detector'
require_relative 'edges'
require_relative 'fan_in'
require_relative 'js_fan_in'
require_relative 'js_complexity'
require_relative 'file_collector'
require_relative 'gate'
require_relative 'loc_counter'
require_relative 'newness'
require_relative 'scorer'
require_relative 'version'

module StudFinder
  # rubocop:disable Metrics/ClassLength
  class CLI
    OUTPUT_FORMATS = %w[table json markdown csv].freeze
    CLASS_RANK = { 'trunk' => 3, 'branch' => 2, 'leaf' => 1 }.freeze
    RESULT_COLUMNS = %w[
      rank language file score evidence class new_file age_days escalation fan_in fan_in_pct fan_out fan_out_pct
      instability instability_pct complexity complexity_pct churn_commits churn_lines churn_pct loc loc_pct max_coupling
      max_coupling_partner coupling_partners coupling_pct coverage
    ].freeze
    MARKDOWN_COLUMNS = %w[
      rank language file score evidence class new_file age_days escalation fan_in fan_out fan_out_pct instability
      complexity
      churn_commits churn_lines churn_pct max_coupling max_coupling_partner coupling_partners coupling_pct coverage
    ].freeze
    WEIGHT_KEYS = %i[fan_in fan_out complexity churn coverage].freeze
    OPTIONAL_WEIGHT_KEYS = %i[interaction coupling].freeze
    DEFAULT_OPTIONS = {
      output: 'table',
      churn_days: 180,
      weights: StudFinder::Scorer::DEFAULT_WEIGHTS,
      custom_weights: false,
      trunk_threshold: 85,
      branch_threshold: 50,
      excludes: [],
      min_files: 20,
      top: nil,
      verbose: false,
      ruby_coverage_path: nil,
      js_coverage_path: nil,
      js_timeout: 60,
      rails_inference: true,
      diff_base: nil,
      only_paths: nil,
      filter_set: nil,
      coupling_threshold: 0.30,
      coupling_min_commits: 5,
      coupling_max_commit_files: 50,
      newness: true,
      new_file_days: StudFinder::Newness::DEFAULT_DAYS,
      new_file_min_commits: StudFinder::Newness::DEFAULT_MIN_COMMITS,
      auto_unshallow: true,
      cli_warnings: []
    }.freeze

    Analysis = Struct.new(:files, :fan_in, :fan_out, :edges, :complexity, :churn_commits, :churn_lines, :loc, :coverage,
                          :coverage_available, :skipped_files, :warnings, :rows, :weights, keyword_init: true)
    Report = Struct.new(:ruby, :javascript, :warnings, keyword_init: true)

    class ValidationError < StandardError; end

    def initialize(argv, stdout: $stdout, stderr: $stderr, stdin: $stdin)
      @argv = argv.dup
      @stdout = stdout
      @stderr = stderr
      @stdin = stdin
      @options = Marshal.load(Marshal.dump(DEFAULT_OPTIONS))
    end

    def self.start(argv = ARGV, stdout: $stdout, stderr: $stderr, stdin: $stdin)
      new(argv, stdout: stdout, stderr: stderr, stdin: stdin).run
    end

    def run
      parser = option_parser
      return run_gate if shift_subcommand?('gate')
      return run_edges_subcommand(parser) if shift_subcommand?('edges')

      run_scan(parser)
    rescue OptionParser::InvalidOption, OptionParser::MissingArgument, OptionParser::InvalidArgument, ValidationError,
           FileCollector::Error, Gate::Error, Churn::Error, Complexity::Error, Coverage::Cobertura::Error,
           Coverage::Detector::Error, Coverage::Lcov::Error, Coverage::Resultset::Error, Diff::Error, Newness::Error,
           Scorer::ValidationError => e
      @stderr.puts e.message
      1
    end

    def shift_subcommand?(name)
      return false unless @argv[0] == name

      @argv.shift
      true
    end

    def run_scan(parser)
      parser.parse!(@argv)
      path = @argv.shift || '.'
      raise ValidationError, "Error: unexpected arguments: #{@argv.join(' ')}" unless @argv.empty?

      @repo_path = File.expand_path(path)
      validate_options!

      result = FileCollector.new(
        path: path,
        excludes: @options[:excludes],
        min_files: @options[:min_files],
        stderr: @stderr
      ).collect
      progress("collecting files... #{result.files.length} found")

      @options[:filter_set] = resolve_filter_set(@repo_path)

      coupling = compute_coupling(@repo_path, result.files)
      analysis = analyze(@repo_path, result.files, result.languages, coupling)
      analysis = warn_if_no_scored_files(analysis)
      emit_results(@repo_path, result, analysis)
      0
    end

    def run_edges_subcommand(parser)
      parser.parse!(@argv)
      target = @argv.shift
      path = @argv.shift || '.'
      raise ValidationError, "Error: unexpected arguments: #{@argv.join(' ')}" unless @argv.empty?

      @repo_path = File.expand_path(path)
      validate_options!
      run_edges(target, path)
    end

    def run_gate
      gate_options = { input: nil, enforce: false }
      OptionParser.new do |opts|
        opts.banner = 'Usage: stud-finder gate [--input FILE] [--enforce]'
        opts.on('--input FILE', 'Read stud-finder JSON output from FILE') { |value| gate_options[:input] = value }
        opts.on('--enforce', 'Exit non-zero when gate findings are present') { gate_options[:enforce] = true }
      end.parse!(@argv)
      raise ValidationError, "Error: unexpected arguments: #{@argv.join(' ')}" unless @argv.empty?

      json = gate_input(gate_options[:input])
      result = Gate.call(json)
      @stdout.puts Gate.markdown(result, enforce: gate_options[:enforce])
      gate_options[:enforce] && result.findings? ? 1 : 0
    end

    def gate_input(input_path)
      return File.read(input_path) if input_path
      return @stdin.read if !@stdin.respond_to?(:tty?) || !@stdin.tty?

      raise ValidationError, 'Error: provide --input FILE or pipe JSON to stdin.'
    rescue Errno::ENOENT
      raise ValidationError, "Error: input file not found: #{input_path}"
    end

    def run_edges(target, path)
      @repo_path = File.expand_path(path)
      result = FileCollector.new(path: path, excludes: @options[:excludes],
                                 min_files: @options[:min_files], stderr: @stderr).collect
      progress("collecting files... #{result.files.length} found")
      analysis = analyze(@repo_path, result.files, result.languages)
      all_rows = analysis.ruby.rows + analysis.javascript.rows
      all_edges = analysis.ruby.edges.merge(analysis.javascript.edges)

      progress("computing temporal coupling (git log, #{@options[:churn_days]} days)...")
      coupling_result = TemporalCoupling.new(
        repo_path: @repo_path,
        files: result.files,
        days: @options[:churn_days],
        min_co_changes: @options[:coupling_min_commits],
        coupling_threshold: @options[:coupling_threshold],
        max_commit_files: @options[:coupling_max_commit_files]
      ).call

      Edges.new(
        target: target, rows: all_rows, edges: all_edges,
        coupling: coupling_result.pairs,
        churn_days: @options[:churn_days],
        coupling_min_commits: @options[:coupling_min_commits],
        coupling_threshold: @options[:coupling_threshold],
        stdout: @stdout, stderr: @stderr
      ).call
    rescue FileCollector::Error, Churn::Error, Complexity::Error, Newness::Error, Scorer::ValidationError => e
      @stderr.puts e.message
      1
    end

    private

    # rubocop:disable Metrics/AbcSize, Metrics/BlockLength, Metrics/MethodLength
    def option_parser
      OptionParser.new do |opts|
        opts.banner = 'Usage: stud-finder [PATH] [OPTIONS]'
        opts.separator ''
        opts.separator 'Options:'

        opts.on('--output FORMAT', OUTPUT_FORMATS,
                'Output format: table, json, markdown, csv (default: table)') do |value|
          @options[:output] = value
        end
        opts.on('--churn-days N', Integer, 'Commit lookback window in days (default: 180)') do |value|
          @options[:churn_days] = value
        end
        opts.on('--weights WEIGHTS',
                'fan_in:F,fan_out:O,complexity:C,churn:H,coverage:V[,interaction:I][,coupling:P]') do |value|
          @options[:weights] = parse_weights(value)
          @options[:custom_weights] = true
        end
        opts.on('--interaction-weight N', Float,
                'fan_in × coverage_risk interaction weight (requires coverage)') do |value|
          raise ValidationError, 'Error: weight values must be between 0.0 and 1.0.' if value.negative? || value > 1.0

          @options[:weights][:interaction] = value
        end
        opts.on('--coupling-weight N', Float, 'temporal coupling weight') do |value|
          raise ValidationError, 'Error: weight values must be between 0.0 and 1.0.' if value.negative? || value > 1.0

          @options[:weights][:coupling] = value
        end
        opts.on('--ruby-coverage PATH', 'Path to a Ruby coverage report (.xml, .info, .json)') do |value|
          @options[:ruby_coverage_path] = value
        end
        opts.on('--js-coverage PATH', 'Path to a JavaScript coverage report (reserved for Phase 2 Chunk B)') do |value|
          @options[:js_coverage_path] = value
        end
        opts.on('--coverage PATH', 'Deprecated alias for --ruby-coverage') do |value|
          @options[:ruby_coverage_path] = value
          @options[:cli_warnings] << 'coverage_flag_deprecated'
          @stderr.puts 'Warning: coverage_flag_deprecated: --coverage is deprecated; use --ruby-coverage.'
        end
        opts.on('--js-timeout N', Integer, 'dependency-cruiser timeout in seconds (default: 60)') do |value|
          @options[:js_timeout] = value
        end
        opts.on('--no-rails-inference', 'Disable Rails association/string fan-in inference') do
          @options[:rails_inference] = false
        end
        opts.on('--trunk-threshold N', Integer,
                'score percentile cutoff for trunk classification, integer 1-99 ' \
                '(default: 85 = top 15% of files by score)') do |value|
          @options[:trunk_threshold] = value
        end
        opts.on('--branch-threshold N', Integer,
                'score percentile cutoff for branch classification, integer 1-99 ' \
                '(default: 50 = top 50% of files by score)') do |value|
          @options[:branch_threshold] = value
        end
        opts.on('--exclude PATTERN', 'Exclude glob pattern (repeatable)') do |value|
          @options[:excludes] << value
        end
        opts.on('--min-files N', Integer, 'Advisory minimum file count (default: 20)') do |value|
          @options[:min_files] = value
        end
        opts.on('--top N', Integer, 'Emit only the top N results') do |value|
          @options[:top] = value
        end
        opts.on('--diff-base REF',
                'Score the full repo but emit only files changed vs REF (merge-base), e.g. origin/staging') do |value|
          @options[:diff_base] = value
        end
        opts.on('--only PATHS',
                'Emit only these comma-separated repo-relative paths (still scored against the full repo)') do |value|
          @options[:only_paths] = value.split(',').map(&:strip).reject(&:empty?)
        end
        opts.on('--coupling-threshold FLOAT', Float,
                'Minimum coupling ratio for edges output (default: 0.30)') do |value|
          @options[:coupling_threshold] = value
        end
        opts.on('--coupling-min-commits N', Integer,
                'Minimum co-change count for edges output (default: 5)') do |value|
          @options[:coupling_min_commits] = value
        end
        opts.on('--coupling-max-commit-files N', Integer,
                'Skip temporal-coupling commits touching more than N files (default: 50, 0 = unlimited)') do |value|
          @options[:coupling_max_commit_files] = value
        end
        opts.on('--new-file-days N', Integer,
                'Treat files first committed within N days as new (default: 30, 0 disables age floor)') do |value|
          @options[:new_file_days] = value
        end
        opts.on('--new-file-min-commits N', Integer,
                'Treat files with fewer than N full-history commits as new ' \
                '(default: 3, 0 disables count floor)') do |value|
          @options[:new_file_min_commits] = value
        end
        opts.on('--no-newness', 'Disable new-file classification rules') do
          @options[:newness] = false
        end
        opts.on('--no-auto-unshallow', 'Disable automatic git fetch --unshallow on shallow clones') do
          @options[:auto_unshallow] = false
        end
        opts.on('--verbose', 'Print suppressed per-file warnings to stderr') do
          @options[:verbose] = true
        end
        opts.on('--version', 'Print version and exit') do
          @stdout.puts StudFinder::VERSION
          exit 0
        end
        opts.on('--help', 'Print help and exit') do
          @stdout.puts opts
          exit 0
        end
      end
    end
    # rubocop:enable Metrics/AbcSize, Metrics/BlockLength, Metrics/MethodLength

    # rubocop:disable Metrics/CyclomaticComplexity, Metrics/PerceivedComplexity
    def parse_weights(value)
      pairs = value.split(',').map do |entry|
        key, raw = entry.split(':', 2)
        raise ValidationError, 'Error: invalid weights format.' if key.nil? || raw.nil? || key.empty? || raw.empty?

        [key.to_sym, Float(raw)]
      rescue ArgumentError
        raise ValidationError, 'Error: weight values must be floats.'
      end

      weights = pairs.to_h
      missing = WEIGHT_KEYS - weights.keys
      extra = weights.keys - WEIGHT_KEYS - OPTIONAL_WEIGHT_KEYS
      unless missing.empty? && extra.empty?
        raise ValidationError,
              'Error: weights must include fan_in, fan_out, complexity, churn, and coverage.'
      end

      weights[:interaction] = 0.0 unless weights.key?(:interaction)
      weights[:coupling] = 0.05 unless weights.key?(:coupling)

      out_of_range = weights.any? { |_key, weight| weight.negative? || weight > 1.0 }
      raise ValidationError, 'Error: weight values must be between 0.0 and 1.0.' if out_of_range

      weights
    end

    # rubocop:enable Metrics/CyclomaticComplexity, Metrics/PerceivedComplexity

    def validate_options!
      validate_threshold!(:trunk_threshold)
      validate_threshold!(:branch_threshold)
      if @options[:branch_threshold] >= @options[:trunk_threshold]
        raise ValidationError, 'Error: branch-threshold must be strictly less than trunk-threshold.'
      end

      raise ValidationError, 'Error: --min-files must be positive.' if @options[:min_files] <= 0
      raise ValidationError, 'Error: --top must be positive.' if @options[:top] && @options[:top] <= 0
      raise ValidationError, 'Error: --churn-days must be positive.' if @options[:churn_days] <= 0
      raise ValidationError, 'Error: --js-timeout must be positive.' if @options[:js_timeout] <= 0

      validate_newness_options!

      raise ValidationError, 'Error: --coupling-min-commits must be positive.' if @options[:coupling_min_commits] <= 0
      if @options[:coupling_max_commit_files].negative?
        raise ValidationError, 'Error: --coupling-max-commit-files must be zero or positive.'
      end
      unless (0.0..1.0).cover?(@options[:coupling_threshold])
        raise ValidationError, 'Error: --coupling-threshold must be between 0.0 and 1.0.'
      end

      validate_coverage_paths!
      validate_filter_options!
      validate_weights! if @options[:custom_weights]
    end

    def validate_newness_options!
      raise ValidationError, 'Error: --new-file-days must be zero or positive.' if @options[:new_file_days].negative?
      return unless @options[:new_file_min_commits].negative?

      raise ValidationError, 'Error: --new-file-min-commits must be zero or positive.'
    end

    def validate_coverage_paths!
      if @options[:ruby_coverage_path] && !File.file?(@options[:ruby_coverage_path])
        raise ValidationError, "Error: coverage file not found: #{@options[:ruby_coverage_path]}"
      end
      return unless @options[:js_coverage_path] && !File.file?(@options[:js_coverage_path])

      raise ValidationError, "Error: JS coverage file not found: #{@options[:js_coverage_path]}"
    end

    def validate_filter_options!
      if @options[:diff_base] && @options[:only_paths]
        raise ValidationError, 'Error: --diff-base and --only are mutually exclusive.'
      end
      return unless @options[:diff_base] && @repo_path

      Diff.new(repo_path: @repo_path, base_ref: @options[:diff_base]).validate_ref!
    rescue Diff::Error => e
      raise ValidationError, e.message
    end

    def validate_threshold!(name)
      value = @options[name]
      return if value.between?(1, 99)

      raise ValidationError, "Error: #{name.to_s.tr('_', '-')} must be between 1 and 99."
    end

    def coverage_available?
      !@options[:ruby_coverage_path].nil? || !@options[:js_coverage_path].nil?
    end

    def validate_weights!
      weights = @options[:weights]
      out_of_range = weights.any? { |_key, weight| !weight.nil? && (weight.negative? || weight > 1.0) }
      raise ValidationError, 'Error: weight values must be between 0.0 and 1.0.' if out_of_range

      if weights[:coverage].positive? && !coverage_available?
        raise ValidationError, 'Error: coverage weight must be 0.0 when no coverage data is provided.'
      end
      if weights.fetch(:interaction, 0.0).positive? && !coverage_available?
        raise ValidationError, 'Error: interaction weight must be 0.0 when no coverage data is provided.'
      end

      active_sum = weights.values.sum
      return if (active_sum - 1.0).abs <= 0.001

      raise ValidationError, format('Error: weights must sum to 1.0; actual sum is %.4f.', active_sum)
    end

    # Computes temporal coupling once over the full collected file set (all languages
    # together, so cross-language co-change is captured) and aggregates each file's
    # partners into { file => { max_coupling: Float, max_coupling_partner: String, partners: Integer } }.
    # max_coupling_partner is the path of the partner that produced max_coupling. Files
    # with no qualifying pairs are simply absent from the hash (scorer treats them as 0).
    def compute_coupling(path, files)
      progress("computing temporal coupling (git log, #{@options[:churn_days]} days)...")
      result = TemporalCoupling.new(
        repo_path: path,
        files: files,
        days: @options[:churn_days],
        min_co_changes: @options[:coupling_min_commits],
        coupling_threshold: @options[:coupling_threshold],
        max_commit_files: @options[:coupling_max_commit_files]
      ).call
      record_coupling_warnings(result.warnings)
      result.pairs.transform_values { |partners| aggregate_partners(partners) }
    end

    def record_coupling_warnings(warnings)
      @options[:cli_warnings].concat(warnings)
      bulk_warning = warnings.find do |warning|
        warning.is_a?(Hash) && warning[:code] == 'temporal_coupling_bulk_commits_skipped'
      end
      return unless @options[:verbose] && bulk_warning

      @stderr.puts "Temporal coupling: skipped #{bulk_warning[:count]} bulk commits " \
                   "(> #{bulk_warning[:max_commit_files]} files)."
    end

    # Reduces a file's partner entries to its strongest-coupling partner.
    # Tie-break is deterministic: highest :coupling, then highest :co_changes,
    # then ascending alphabetical :path — so identical inputs always pick the same partner.
    def aggregate_partners(partners)
      top = partners.min_by { |entry| [-entry[:coupling], -entry[:co_changes], entry[:path]] }
      {
        max_coupling: top ? top[:coupling] : 0.0,
        max_coupling_partner: top ? top[:path] : nil,
        partners: partners.length
      }
    end

    def analyze(path, files, languages, coupling = nil)
      ruby_files = files.select { |file| languages[file] == :ruby }
      js_files = files.select { |file| %i[javascript typescript].include?(languages[file]) }

      ruby_analysis = ruby_files.empty? ? empty_analysis : analyze_ruby(path, ruby_files, coupling)
      javascript_analysis = js_files.empty? ? empty_analysis : analyze_javascript(path, js_files, languages, coupling)

      progress('done')
      Report.new(ruby: ruby_analysis, javascript: javascript_analysis,
                 warnings: (ruby_analysis.warnings + javascript_analysis.warnings + @options[:cli_warnings]).uniq)
    end

    def analyze_ruby(path, files, coupling = nil)
      progress('computing Ruby fan_in + fan_out (rubocop-ast)...')
      fan_in_result = FanIn.new(repo_path: path, files: files, stderr: @stderr,
                                rails_inference: @options[:rails_inference]).call

      progress('computing Ruby complexity (rubocop)...')
      complexity_result = Complexity.new(repo_path: path, files: files, stderr: @stderr).call
      analysis_files = files - complexity_result.skipped_files

      progress("computing Ruby churn (git log, #{@options[:churn_days]} days)...")
      churn_result = Churn.new(repo_path: path, files: analysis_files, days: @options[:churn_days],
                               stderr: @stderr).call

      loc = LocCounter.new(repo_path: path, files: analysis_files).call
      score_group(analysis_files, fan_in_result.counts, fan_in_result.fan_out_counts, fan_in_result.edges,
                  complexity_result.counts, churn_result, loc, complexity_result.skipped_files,
                  ruby_coverage(path, analysis_files),
                  language_by_file: analysis_files.to_h { |file| [file, :ruby] }, coupling: coupling)
    end

    def analyze_javascript(path, files, languages, coupling = nil)
      progress('computing JavaScript fan_in + fan_out (dependency-cruiser)...')
      fan_in_result = JsFanIn.new(repo_path: path, files: files, js_timeout: @options[:js_timeout],
                                  stderr: @stderr).call
      progress('computing JavaScript complexity (eslint)...')
      complexity_result = JsComplexity.new(repo_path: path, files: files, js_timeout: @options[:js_timeout],
                                           stderr: @stderr).call
      churn_result = Churn.new(repo_path: path, files: files, days: @options[:churn_days], stderr: @stderr).call
      loc = LocCounter.new(repo_path: path, files: files).call
      score_group(files, fan_in_result.counts, fan_in_result.fan_out_counts, fan_in_result.edges,
                  complexity_result.counts, churn_result, loc, [], js_coverage(path, files),
                  language_by_file: languages, extra_warnings: fan_in_result.warnings + complexity_result.warnings,
                  coupling: coupling)
    end

    # rubocop:disable Metrics/ParameterLists
    def score_group(files, fan_in, fan_out, edges, complexity, churn_result, loc, skipped_files, coverage_payload,
                    language_by_file: {}, extra_warnings: [], coupling: nil)
      progress("normalizing + scoring #{files.length} files...")
      coverage_result, coverage_parser = coverage_payload
      loc_pct = loc_percentiles_by_language(files, loc, language_by_file)
      scorer = Scorer.new(files: files, fan_in: fan_in, fan_out: fan_out, complexity: complexity,
                          churn: churn_result.counts, churn_lines: churn_result.line_counts, loc: loc, loc_pct: loc_pct,
                          coverage: coverage_result, weights: @options[:weights],
                          branch_threshold: @options[:branch_threshold], trunk_threshold: @options[:trunk_threshold],
                          coupling: coupling)
      warnings = extra_warnings.dup
      warnings << 'coverage_unavailable' unless coverage_result
      warnings << 'coverage_partial' if coverage_parser&.missing_files&.any?
      warnings << 'zero_churn_majority' if churn_result.zero_inflated
      warnings << 'files_skipped' if skipped_files.any?
      warnings << 'small_repo' if files.length < @options[:min_files]
      emit_scoring_note(scorer, coverage_result)
      scored_rows = scorer.call
      warnings.concat(scorer.warnings)
      rows = apply_newness(files, edges, scored_rows, coverage_result)
             .map { |row| with_language(row, language_by_file) }
      rows = sort_and_rank_rows(rows, files)
      Analysis.new(
        files: files, fan_in: fan_in, fan_out: fan_out, edges: edges, complexity: complexity,
        churn_commits: churn_result.churn_commits, churn_lines: churn_result.churn_lines, loc: loc,
        coverage: coverage_result, coverage_available: !coverage_result.nil?, skipped_files: skipped_files,
        warnings: warnings.uniq, rows: rows,
        weights: scorer.normalized_weights
      )
    end
    # rubocop:enable Metrics/ParameterLists

    def apply_newness(files, edges, rows, coverage_result = nil)
      metadata = if @options[:newness] && Newness.shallow_repository?(@repo_path)
                   if @options[:auto_unshallow]
                     attempt_unshallow(files)
                   else
                     @options[:cli_warnings] << Newness::SHALLOW_CLONE_WARNING
                     Newness.disabled_metadata(files)
                   end
                 elsif @options[:newness]
                   Newness.new(repo_path: @repo_path, files: files, days: @options[:new_file_days],
                               min_commits: @options[:new_file_min_commits]).call
                 else
                   Newness.disabled_metadata(files)
                 end
      Newness.apply(rows: rows, edges: edges, metadata: metadata, coverage: coverage_result)
    end

    def attempt_unshallow(files)
      progress('auto-unshallow: fetching full history to compute evidence (use --no-auto-unshallow to skip)')
      Open3.popen3('git', '-C', @repo_path, 'fetch', '--unshallow') do |_stdin, _stdout, _stderr, wait_thread|
        unless wait_thread.join(45)
          Process.kill('TERM', wait_thread.pid)
          Process.kill('KILL', wait_thread.pid) unless wait_thread.join(5)
          raise 'unshallow timed out'
        end
        raise 'unshallow failed' unless wait_thread.value.success?

        Newness.new(repo_path: @repo_path, files: files, days: @options[:new_file_days],
                    min_commits: @options[:new_file_min_commits]).call
      end
    rescue StandardError
      @options[:cli_warnings] << Newness::SHALLOW_CLONE_WARNING
      @options[:cli_warnings] << Newness::UNSHALLOW_FAILED_WARNING
      Newness.disabled_metadata(files)
    end

    def loc_percentiles_by_language(files, loc, language_by_file)
      files.group_by { |file| language_by_file.fetch(file) }.values.reduce({}) do |pct, language_files|
        pct.merge(Normalizer.percentile_rank(loc, language_files))
      end
    end

    def with_language(row, language_by_file)
      row.merge(language: language_by_file.fetch(row[:path]).to_s)
    end

    def ruby_coverage(path, files)
      return [nil, nil] unless @options[:ruby_coverage_path]

      parser = Coverage::Detector.for(path: @options[:ruby_coverage_path], files: files, project_root: path)
      [parser.call, parser]
    end

    def js_coverage(path, files)
      return [nil, nil] unless @options[:js_coverage_path]

      parser = Coverage::Detector.for(path: @options[:js_coverage_path], files: files, project_root: path)
      [parser.call, parser]
    end

    def emit_scoring_note(scorer, coverage_result)
      if coverage_result
        @stderr.puts coverage_scoring_note(weights: scorer.normalized_weights)
      else
        @stderr.puts scoring_note(weights: scorer.normalized_weights, stderr: true)
      end
    end

    def emit_results(path, result, analysis)
      ruby_rows = limited_rows(analysis.ruby.rows)
      javascript_rows = limited_rows(analysis.javascript.rows)

      case @options[:output]
      when 'json'
        emit_json(path, analysis, ruby_rows, javascript_rows)
      when 'markdown'
        emit_markdown(analysis, ruby_rows, javascript_rows)
      when 'csv'
        emit_csv(ruby_rows + javascript_rows)
      else
        emit_table(path, result, analysis, ruby_rows, javascript_rows)
      end
    end

    # Resolves the optional output filter to a Set of repo-relative paths, or nil.
    # The filter is applied at emit time only (see #limited_rows) so the full repo
    # is still scored — fan_in counts and percentiles stay correct.
    def resolve_filter_set(path)
      paths =
        if @options[:diff_base]
          Diff.new(repo_path: path, base_ref: @options[:diff_base]).changed_paths
        else
          @options[:only_paths]
        end
      return nil unless paths

      # Diff (and documented --only) paths are repo-root-relative, but row paths are
      # relative to the analysis root (FileCollector). Rebase so they compare equal
      # when PATH is a subdirectory; a no-op when PATH is the repo root.
      paths = rebase_to_analysis_root(paths, path)

      if paths.empty?
        @stderr.puts 'Note: diff contains no changed files. Nothing to filter.'
        @options[:cli_warnings] << 'diff_filter_empty'
      end
      Set.new(paths)
    end

    # Strips the analysis-root prefix from repo-root-relative filter paths and drops
    # any path outside the analysis root. Returns paths unchanged when the analysis
    # root is the repo root (or the toplevel can't be resolved).
    def rebase_to_analysis_root(paths, analysis_path)
      toplevel = git_toplevel(analysis_path)
      return paths if toplevel.nil?

      # realpath on both sides so symlinked roots (e.g. macOS /var -> /private/var)
      # don't defeat the prefix comparison.
      analysis_abs = File.realpath(analysis_path)
      return paths if analysis_abs == toplevel

      prefix = Pathname.new(analysis_abs).relative_path_from(Pathname.new(toplevel)).to_s
      return paths if prefix.empty? || prefix == '.' || prefix.start_with?('..')

      prefix += '/'
      paths.select { |p| p.start_with?(prefix) }.map { |p| p.delete_prefix(prefix) }
    rescue Errno::ENOENT
      paths
    end

    def git_toplevel(analysis_path)
      stdout, _stderr, status = Open3.capture3(
        'git', '-C', File.expand_path(analysis_path), 'rev-parse', '--show-toplevel'
      )
      status.success? ? File.realpath(stdout.strip) : nil
    end

    def warn_if_no_scored_files(analysis)
      return analysis unless @options[:filter_set] && !@options[:filter_set].empty?

      scored = Set.new(analysis.ruby.files + analysis.javascript.files)
      return analysis if @options[:filter_set].intersect?(scored)

      @stderr.puts 'Note: no scored files matched the diff. ' \
                   'The PR may only touch unscorable files (docs, config, migrations, etc.).'
      Report.new(ruby: analysis.ruby, javascript: analysis.javascript,
                 warnings: (analysis.warnings + ['diff_no_scored_files']).uniq)
    end

    def sort_and_rank_rows(rows, files)
      original_index = files.each_with_index.to_h
      sorted = rows.sort_by do |row|
        [-CLASS_RANK.fetch(row[:classification], 0), -row[:score], original_index.fetch(row[:path])]
      end
      sorted.map.with_index(1) { |row, rank| row.merge(rank: rank) }
    end

    def limited_rows(rows)
      filtered = @options[:filter_set] ? rows.select { |row| @options[:filter_set].include?(row[:path]) } : rows
      @options[:top] ? filtered.first(@options[:top]) : filtered
    end

    def filter_note
      return unless @options[:filter_set]

      if @options[:diff_base]
        "Filtered to files changed vs #{@options[:diff_base]} (ranks are against the full repo)."
      else
        'Filtered to --only paths (ranks are against the full repo).'
      end
    end

    def empty_analysis
      Analysis.new(
        files: [], fan_in: {}, fan_out: {}, edges: {}, complexity: {}, churn_commits: {}, churn_lines: {}, loc: {},
        coverage: nil, coverage_available: false, skipped_files: [], warnings: [], rows: [], weights: nil
      )
    end

    def emit_markdown_section(title, rows)
      @stdout.puts "### #{title}"
      @stdout.puts
      @stdout.puts "| #{MARKDOWN_COLUMNS.join(' | ')} |"
      @stdout.puts "| #{MARKDOWN_COLUMNS.map { '---' }.join(' | ')} |"
      rows.each { |row| @stdout.puts markdown_row(row) }
      @stdout.puts
    end

    def emit_table_section(title, rows)
      @stdout.puts title
      @stdout.puts ' rank  language    file                                            score  evidence  class   new  ' \
                   'age_days  ' \
                   'escalation      fan_in  fan_out  instability  complexity  churn_commits  churn_lines  ' \
                   'churn_pct    loc  loc_pct  ' \
                   'max_coupling  max_coupling_partner                      coupling_partners  coupling_pct  ' \
                   'coverage'
      rows.each { |row| @stdout.puts table_row(row) }
      @stdout.puts
    end

    def emit_csv(rows)
      @stdout << CSV.generate_line(RESULT_COLUMNS)
      rows.each do |row|
        @stdout << CSV.generate_line(csv_file(row))
      end
    end

    def emit_json(path, analysis, ruby_rows, javascript_rows)
      @stdout.puts JSON.generate(
        meta: json_meta(path, analysis),
        warnings: Warnings.normalize(analysis.warnings),
        ruby: ruby_rows.map { |row| json_file(row) },
        javascript: javascript_rows.map { |row| json_file(row) }
      )
    end

    def json_meta(path, analysis)
      meta = {
        repo: path,
        analyzed_at: Time.now.utc.iso8601,
        churn_days: @options[:churn_days],
        file_count: analysis.ruby.files.length + analysis.javascript.files.length,
        files_skipped: analysis.ruby.skipped_files.length + analysis.javascript.skipped_files.length,
        formula: json_formula(analysis),
        weights: json_weights(analysis.ruby.weights || analysis.javascript.weights),
        schema_version: 1,
        warnings: Warnings.normalize(analysis.warnings)
      }
      meta[:filtered] = true if @options[:filter_set]
      meta[:diff_base] = @options[:diff_base] if @options[:diff_base]
      meta[:only_paths] = @options[:only_paths] if @options[:only_paths]
      meta
    end

    def emit_markdown(analysis, ruby_rows, javascript_rows)
      @stdout.puts "## stud-finder — #{Time.now.utc.strftime('%Y-%m-%d')}"
      @stdout.puts
      file_count = analysis.ruby.files.length + analysis.javascript.files.length
      weights = analysis.ruby.weights || analysis.javascript.weights
      md_formula = formula_label(coverage_available: report_coverage_available?(analysis),
                                 coupling_available: weights[:coupling])
      @stdout.puts "> #{md_formula}. Churn window: #{@options[:churn_days]} days. #{file_count} files analyzed."
      note = filter_note
      if note
        @stdout.puts
        @stdout.puts "> #{note}"
      end
      emit_markdown_section('Ruby', ruby_rows)
      emit_markdown_section('JavaScript/TypeScript', javascript_rows)
      @stdout.puts
      @stdout.puts '*fan_in is a static approximation — dynamic references not counted.*'
    end

    def markdown_row(row)
      values = [
        row[:rank], row[:language], row[:path], format_score(row[:score]), format_evidence(row[:evidence]),
        row[:classification], row[:new_file], row[:age_days], row[:escalation], row[:fan_in], row[:fan_out],
        format_score(row[:fan_out_pct]),
        format_score(row[:instability]), row[:complexity], row[:churn_commits], row[:churn_lines],
        format_score(row[:churn_pct]), format_score(row[:max_coupling]), row[:max_coupling_partner],
        row[:coupling_partners], format_score(row[:coupling_pct]), format_coverage(row[:coverage])
      ]
      "| #{values.join(' | ')} |"
    end

    def emit_table(path, result, analysis, ruby_rows, javascript_rows)
      coverage_available = report_coverage_available?(analysis)
      weights = analysis.ruby.weights || analysis.javascript.weights
      formula = formula_label(coverage_available: coverage_available, coupling_available: weights[:coupling])
      @stdout.puts "stud-finder — #{path} (#{@options[:churn_days]}-day churn, #{formula})"
      unless coverage_available
        @stdout.puts scoring_note(weights: analysis.ruby.weights || analysis.javascript.weights,
                                  stderr: false)
      end
      note = filter_note
      @stdout.puts note if note
      @stdout.puts
      emit_table_section('Ruby', ruby_rows)
      emit_table_section('JavaScript/TypeScript', javascript_rows)
      @stdout.puts
      @stdout.puts footer(result, analysis)
      @stdout.puts 'fan_in is a static approximation — dynamic references (const_get, send, metaprogramming) ' \
                   'not counted.'
    end

    def json_file(row)
      {
        rank: row[:rank],
        language: row[:language],
        path: row[:path],
        score: row[:score],
        evidence: row[:evidence],
        class: row[:classification],
        new_file: row[:new_file],
        age_days: row[:age_days],
        escalation: row[:escalation],
        fan_in: row[:fan_in],
        fan_in_pct: row[:fan_in_pct],
        fan_out: row[:fan_out],
        fan_out_pct: row[:fan_out_pct],
        instability: row[:instability],
        instability_pct: row[:instability_pct],
        complexity: row[:complexity],
        complexity_pct: row[:complexity_pct],
        churn_commits: row[:churn_commits],
        churn_lines: row[:churn_lines],
        churn_pct: row[:churn_pct],
        loc: row[:loc],
        loc_pct: row[:loc_pct],
        max_coupling: row[:max_coupling],
        max_coupling_partner: row[:max_coupling_partner],
        coupling_partners: row[:coupling_partners],
        coupling_pct: row[:coupling_pct],
        coverage: row[:coverage]
      }
    end

    def csv_file(row)
      [
        row[:rank],
        row[:language],
        row[:path],
        format_score(row[:score]),
        format_evidence(row[:evidence]),
        row[:classification],
        row[:new_file],
        row[:age_days],
        row[:escalation],
        row[:fan_in],
        format_score(row[:fan_in_pct]),
        row[:fan_out],
        format_score(row[:fan_out_pct]),
        format_score(row[:instability]),
        format_score(row[:instability_pct]),
        row[:complexity],
        format_score(row[:complexity_pct]),
        row[:churn_commits],
        row[:churn_lines],
        format_score(row[:churn_pct]),
        row[:loc],
        format_score(row[:loc_pct]),
        format_score(row[:max_coupling]),
        row[:max_coupling_partner],
        row[:coupling_partners],
        format_score(row[:coupling_pct]),
        row[:coverage] || ''
      ]
    end

    def report_coverage_available?(analysis)
      analysis.ruby.coverage_available || analysis.javascript.coverage_available
    end

    def json_formula(analysis)
      weights = analysis.ruby.weights || analysis.javascript.weights
      formula_label(
        coverage_available: report_coverage_available?(analysis),
        coupling_available: weights[:coupling]
      )
    end

    def formula_label(coverage_available:, coupling_available:)
      coupling = coupling_available.to_f.positive?
      if coverage_available
        coupling ? '5-factor + coupling' : '5-factor'
      else
        coupling ? '4-factor + coupling' : '4-factor'
      end
    end

    def json_weights(weights)
      {
        fan_in: weights[:fan_in].round(4),
        fan_out: weights[:fan_out].round(4),
        complexity: weights[:complexity].round(4),
        churn: weights[:churn].round(4),
        coverage: weights[:coverage]&.round(4),
        interaction: weights[:interaction]&.round(4),
        coupling: weights[:coupling]&.round(4)
      }
    end

    def coverage_scoring_note(weights:)
      label = formula_label(coverage_available: true, coupling_available: weights[:coupling])
      "Note: coverage data available. Score uses #{label} formula."
    end

    def scoring_note(weights:, stderr:)
      label = formula_label(coverage_available: false, coupling_available: weights[:coupling])
      if weights[:coupling]
        if stderr
          format("Note: coverage data not available. Score uses #{label} formula (fan_in %<fan_in>.2f, " \
                 'fan_out %<fan_out>.2f, complexity %<complexity>.2f, churn %<churn>.2f, ' \
                 'coupling %<coupling>.2f).', **weights)
        else
          format('Note: coverage data not available. Score uses fan_in %<fan_in>.2f, fan_out %<fan_out>.2f, ' \
                 'complexity %<complexity>.2f, churn %<churn>.2f, coupling %<coupling>.2f.', **weights)
        end
      elsif stderr
        format("Note: coverage data not available. Score uses #{label} formula (fan_in %<fan_in>.2f, " \
               'fan_out %<fan_out>.2f, complexity %<complexity>.2f, churn %<churn>.2f).', **weights)
      else
        format('Note: coverage data not available. Score uses fan_in %<fan_in>.2f, fan_out %<fan_out>.2f, ' \
               'complexity %<complexity>.2f, churn %<churn>.2f.', **weights)
      end
    end

    def progress(message)
      @stderr.puts "stud-finder → #{message}"
    end

    def footer(result, analysis)
      file_count = analysis.ruby.files.length + analysis.javascript.files.length
      skipped_count = analysis.ruby.skipped_files.length + analysis.javascript.skipped_files.length
      parts = ["#{file_count} files analyzed."]
      parts << "#{skipped_count} files skipped (parse errors — run --verbose to see)." if skipped_count.positive?
      parts << "#{result.default_excluded_count} files excluded by default rules."
      parts.join(' ')
    end

    def format_score(score)
      format('%.4f', score)
    end

    def format_coverage(coverage)
      return 'n/a' if coverage.nil?

      return coverage if coverage.is_a?(String)

      "#{(coverage * 100).round}%"
    end

    def format_evidence(evidence)
      return nil if evidence.nil?

      format_score(evidence)
    end

    def table_row(row)
      format('%<rank>5d  %<language>-10s  %<path>-45s  %<score>6s  %<evidence>8s  ' \
             '%<classification>-6s  %<new_file>5s  ' \
             '%<age_days>8d  %<escalation>-15s  %<fan_in>6d  %<fan_out>7d  %<instability>11s  %<complexity>10d  ' \
             '%<churn_commits>13d  %<churn_lines>11d  %<churn_pct>9s  %<loc>5d  %<loc_pct>7s  ' \
             '%<max_coupling>12s  %<max_coupling_partner>-40s  %<coupling_partners>17d  ' \
             '%<coupling_pct>12s  %<coverage>8s',
             rank: row[:rank], language: row[:language], path: row[:path], score: format_score(row[:score]),
             evidence: format_evidence(row[:evidence]) || 'n/a', classification: row[:classification],
             new_file: row[:new_file].to_s, age_days: row[:age_days].to_i,
             escalation: row[:escalation],
             fan_in: row[:fan_in], fan_out: row[:fan_out], instability: format_score(row[:instability]),
             complexity: row[:complexity],
             churn_commits: row[:churn_commits], churn_lines: row[:churn_lines],
             churn_pct: format_score(row[:churn_pct]), loc: row[:loc], loc_pct: format_score(row[:loc_pct]),
             max_coupling: format_score(row[:max_coupling]),
             max_coupling_partner: truncate_partner(row[:max_coupling_partner]),
             coupling_partners: row[:coupling_partners], coupling_pct: format_score(row[:coupling_pct]),
             coverage: format_coverage(row[:coverage]))
    end

    # Keeps the partner path readable in the fixed-width table. The partner name is
    # the actionable info, so on overflow we keep the rightmost ~40 chars (the
    # basename and nearest parent dirs) with a leading ellipsis marker.
    def truncate_partner(partner)
      partner = partner.to_s
      return partner if partner.length <= 40

      "...#{partner[-37..]}"
    end
  end
  # rubocop:enable Metrics/ClassLength
end
