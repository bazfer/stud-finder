# frozen_string_literal: true

require 'json'
require 'open3'
require 'timeout'
require_relative 'js_worker_client'

module StudFinder
  class JsComplexity
    include JsWorkerClient

    Result = Struct.new(:counts, :warnings, keyword_init: true)

    ESLINT_MISSING = 'js_eslint_missing'
    ESLINT_FAILED = 'js_eslint_failed'
    ESLINT_MALFORMED = 'js_eslint_malformed'
    TS_PARSER_MISSING = 'js_ts_parser_missing'
    TIMEOUT = 'js_eslint_timeout'
    BATCH_SIZE = 500
    TS_EXTENSIONS = %w[.ts .tsx].freeze

    def initialize(repo_path:, files:, js_timeout: 60, stderr: $stderr, cache_dir: nil, worker: nil)
      @repo_path = File.expand_path(repo_path)
      @files = files
      @js_timeout = js_timeout
      @stderr = stderr
      @worker = worker
      @warnings = []
      @cache_dir = cache_dir
    end

    def call
      return worker_call if @worker && @cache_dir

      eslint = eslint_binary
      return missing_eslint unless eslint

      major = eslint_major(eslint)
      return missing_eslint unless major

      ts_parser = ts_parser_available?
      warn_once(TS_PARSER_MISSING) if ts_files? && !ts_parser

      counts = zero_counts
      analyzable_files(ts_parser).each_slice(BATCH_SIZE) do |batch|
        counts.merge!(run_batch(eslint, major, ts_parser, batch)) { |_file, old, new| [old, new].max }
      end

      Result.new(counts: counts, warnings: @warnings)
    end

    private

    def worker_operation
      'complexity'
    end

    def worker_result(payload)
      Result.new(counts: payload.fetch('counts'), warnings: [])
    end

    def eslint_binary
      local = File.join(@repo_path, 'node_modules/.bin/eslint')
      return local if File.executable?(local)

      ENV.fetch('PATH', '').split(File::PATH_SEPARATOR).each do |dir|
        candidate = File.join(dir, 'eslint')
        return 'eslint' if File.file?(candidate) && File.executable?(candidate)
      end

      nil
    end

    def eslint_major(eslint)
      stdout, _stderr, status = Open3.capture3(eslint, '--version')
      return nil unless status.success?

      stdout[/v?(\d+)/, 1]&.to_i
    rescue Errno::ENOENT
      nil
    end

    def ts_parser_available?
      script = <<~JS
        const parserPath = require.resolve('@typescript-eslint/parser', { paths: [process.cwd()] });
        require(parserPath);
      JS
      _stdout, _stderr, status = Open3.capture3('node', '-e', script, chdir: @repo_path)
      status.success?
    rescue Errno::ENOENT
      false
    end

    def ts_files?
      @files.any? { |file| TS_EXTENSIONS.include?(File.extname(file)) }
    end

    def analyzable_files(ts_parser)
      return @files if ts_parser

      @files.reject { |file| TS_EXTENSIONS.include?(File.extname(file)) }
    end

    def run_batch(eslint, major, ts_parser, batch)
      temp_config = nil
      args = [eslint]
      if major >= 9
        temp_config = write_flat_config(ts_parser)
        args.push('--config', temp_config)
      else
        args.concat(v8_flags(ts_parser))
      end
      args.push('--rule', '{"complexity":["error",0]}', '--format', 'json')
      args.concat(batch)

      stdout, _stderr, status = Timeout.timeout(@js_timeout) do
        Open3.capture3(*args, chdir: @repo_path)
      end
      return degraded_batch(batch, ESLINT_FAILED) if status.exitstatus == 2

      parse_output(stdout, batch)
    rescue Timeout::Error
      degraded_batch(batch, TIMEOUT)
    ensure
      File.delete(temp_config) if temp_config && File.exist?(temp_config)
    end

    def v8_flags(ts_parser)
      flags = ['--no-eslintrc', '--resolve-plugins-relative-to', '.',
               '--parser-options=ecmaVersion:2022,sourceType:module']
      flags.push('--parser', '@typescript-eslint/parser') if ts_parser
      flags
    end

    def write_flat_config(ts_parser)
      path = "/tmp/stud-finder-eslint-#{Process.pid}-#{object_id}.config.mjs"
      parser_setup = if ts_parser
                       <<~JS
                         import { createRequire } from 'node:module';
                         const require = createRequire(#{JSON.generate(File.join(@repo_path, 'package.json'))});
                         const tsParser = require('@typescript-eslint/parser');
                       JS
                     else
                       ''
                     end
      parser_option = ts_parser ? ', parser: tsParser' : ''
      File.write(path, <<~JS)
        #{parser_setup}
        export default [{
          files: ['**/*.{js,jsx,ts,tsx}'],
          languageOptions: { ecmaVersion: 2022, sourceType: 'module'#{parser_option} },
          rules: { complexity: ['error', 0] }
        }];
      JS
      path
    end

    def parse_output(stdout, batch)
      return degraded_batch(batch, ESLINT_MALFORMED) unless stdout.strip.start_with?('[')

      parse_json(stdout)
    rescue JSON::ParserError, KeyError, NoMethodError, TypeError
      degraded_batch(batch, ESLINT_MALFORMED)
    end

    def parse_json(stdout)
      payload = JSON.parse(stdout)
      raise TypeError, 'expected ESLint JSON array' unless payload.is_a?(Array)

      payload.each_with_object({}) do |file_result, counts|
        file = normalize_path(file_result.fetch('filePath'))
        complexities = Array(file_result['messages']).filter_map do |message|
          message['message'].to_s[/complexity of (\d+)/, 1]&.to_i
        end
        counts[file] = complexities.max if complexities.any?
      end
    end

    def parse_text(stdout)
      stdout.each_line.with_object({}) do |line, counts|
        match = line.match(/(.+): line \d+.*complexity of (\d+)/)
        next unless match

        file = normalize_path(match[1])
        counts[file] = [counts.fetch(file, 0), match[2].to_i].max
      end
    end

    def normalize_path(path)
      expanded = File.expand_path(path, @repo_path)
      prefix = "#{@repo_path}/"
      return expanded.delete_prefix(prefix) if expanded.start_with?(prefix)

      real_prefix = "#{File.realpath(@repo_path)}/"
      return expanded.delete_prefix(real_prefix) if expanded.start_with?(real_prefix)

      path.delete_prefix('./')
    rescue Errno::ENOENT
      path.delete_prefix('./')
    end

    def missing_eslint
      warn_once(ESLINT_MISSING)
      Result.new(counts: zero_counts, warnings: @warnings)
    end

    def degraded_batch(batch, code)
      warn_once(code)
      batch.to_h { |file| [file, 0] }
    end

    def zero_counts
      @files.to_h { |file| [file, 0] }
    end

    def warn_once(code)
      return if @warnings.include?(code)

      @warnings << code
      @stderr.puts "Warning: #{code}"
    end
  end
end
