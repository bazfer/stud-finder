# frozen_string_literal: true

require 'json'
require 'open3'
require 'set'
require 'timeout'

module StudFinder
  class JsFanIn
    Result = Struct.new(:counts, :fan_out_counts, :edges, :warnings, keyword_init: true)

    TOOL_MISSING = 'js_tools_missing'
    TIMEOUT = 'js_depcruise_timeout'
    DEPCRUISE_NO_CONFIG = 'js_depcruise_no_config'
    DEPCRUISE_FAILED = 'js_depcruise_failed'

    FALLBACK_SUCCESS_MESSAGE = 'configured depcruise failed; retried with --no-config; fan_in may be undercounted'

    def initialize(repo_path:, files:, js_timeout: 60, stderr: $stderr)
      @repo_path = File.expand_path(repo_path)
      @files = files
      @js_timeout = js_timeout
      @stderr = stderr
    end

    def call
      return missing_tools unless node_available?

      depcruise = depcruise_binary
      return missing_tools unless depcruise

      stdout, stderr, status, warnings = run_depcruise(depcruise)
      return depcruise_failed(stderr) unless status.success?

      counts, fan_out_counts, edges = parse(stdout)
      warnings.each { |warning| warn(warning.fetch(:code), warning.fetch(:message)) }
      Result.new(counts: counts, fan_out_counts: fan_out_counts, edges: edges, warnings: warnings)
    rescue Timeout::Error
      warn(TIMEOUT)
      Result.new(counts: zero_counts, fan_out_counts: zero_counts, edges: empty_edges, warnings: [TIMEOUT])
    rescue JSON::ParserError, KeyError, TypeError => e
      depcruise_failed("malformed dependency-cruiser JSON: #{e.message}")
    end

    private

    def node_available?
      _stdout, _stderr, status = Open3.capture3('node', '--version')
      status.success?
    rescue Errno::ENOENT
      false
    end

    def depcruise_binary
      local = File.join(@repo_path, 'node_modules/.bin/depcruise')
      return local if File.executable?(local)

      ENV.fetch('PATH', '').split(File::PATH_SEPARATOR).each do |dir|
        candidate = File.join(dir, 'depcruise')
        return 'depcruise' if File.file?(candidate) && File.executable?(candidate)
      end

      nil
    end

    def run_depcruise(depcruise)
      Timeout.timeout(@js_timeout) do
        primary_stdout, primary_stderr, primary_status = Open3.capture3(depcruise, '--output-type', 'json', '.',
                                                                        chdir: @repo_path)
        return [primary_stdout, primary_stderr, primary_status, []] if primary_status.success?

        retry_stdout, retry_stderr, retry_status = Open3.capture3(
          depcruise, '--output-type', 'json', '.', '--no-config', chdir: @repo_path
        )
        if retry_status.success?
          [retry_stdout, retry_stderr, retry_status,
           [{ code: DEPCRUISE_NO_CONFIG, message: fallback_success_message(primary_stderr) }]]
        else
          [primary_stdout, primary_stderr, primary_status, []]
        end
      end
    end

    def parse(stdout)
      payload = JSON.parse(stdout)
      file_set = @files.to_h { |file| [file, true] }
      counts = zero_counts
      fan_out_counts = zero_counts
      dependents = @files.to_h { |file| [file, []] }
      dependencies = @files.to_h { |file| [file, []] }
      seen_edges = Set.new

      Array(payload.fetch('modules')).each do |mod|
        source = normalize_path(mod.fetch('source'))
        next unless file_set[source]

        Array(mod['dependencies']).each do |dependency|
          target = normalize_path(dependency['resolved'].to_s)
          next unless file_set[target]
          next if target == source
          next unless seen_edges.add?([source, target])

          counts[target] += 1
          fan_out_counts[source] += 1
          dependents[target] << source
          dependencies[source] << target
        end
      end

      edges = @files.to_h do |file|
        [file, { dependents: dependents[file], dependencies: dependencies[file] }]
      end

      [counts, fan_out_counts, edges]
    end

    def normalize_path(path)
      path.delete_prefix('./')
    end

    def missing_tools
      warn(TOOL_MISSING)
      Result.new(counts: zero_counts, fan_out_counts: zero_counts, edges: empty_edges, warnings: [TOOL_MISSING])
    end

    def depcruise_failed(stderr)
      detail = first_line(stderr)
      message = detail.empty? ? nil : detail
      warn(DEPCRUISE_FAILED, message)
      Result.new(
        counts: zero_counts,
        fan_out_counts: zero_counts,
        edges: empty_edges,
        warnings: [{ code: DEPCRUISE_FAILED, message: message }]
      )
    end

    def fallback_success_message(primary_stderr)
      detail = first_line(primary_stderr)
      return FALLBACK_SUCCESS_MESSAGE if detail.empty?

      "#{FALLBACK_SUCCESS_MESSAGE}: #{detail}"
    end

    def first_line(text)
      text.to_s.lines.first.to_s.strip
    end

    def zero_counts
      @files.to_h { |file| [file, 0] }
    end

    def empty_edges
      @files.to_h { |file| [file, { dependents: [], dependencies: [] }] }
    end

    def warn(code, message = nil)
      suffix = message.to_s.empty? ? '' : ": #{message}"
      @stderr.puts "Warning: #{code}#{suffix}"
    end
  end
end
