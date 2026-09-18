# frozen_string_literal: true

require 'stringio'
require_relative 'cli'
require_relative 'js_worker'

module StudFinder
  # Repeated complete scans with one JavaScript worker. Each scan re-discovers
  # files and validates content; callers need not supply a trustworthy dirty set.
  class Session
    def initialize(argv, stderr: $stderr)
      @argv = argv.dup
      @stderr = stderr
      @worker = JsWorker.new
      @directory_index = DirectoryIndex.new
      @mutex = Mutex.new
    end

    attr_reader :generation

    def scan(extra_args: [])
      @mutex.synchronize do
        unless @primed
          RuboCop::ResultCache.source_checksum
          RuboCop::ConfigLoader.default_configuration
          @primed = true
        end
        3.times do
          output = StringIO.new
          cli = CLI.new(@argv + extra_args + ['--output', 'json'], stdout: output,
                                                                   stderr: @stderr, js_worker: @worker,
                                                                   directory_index: @directory_index)
          status = cli.run
          next if status == 2

          raise CLI::ValidationError, 'Session scan failed; see diagnostics.' unless status.zero?

          @generation = cli.source_generation
          return JSON.parse(output.string)
        end
        raise CLI::ValidationError, 'Sources are still changing; no consistent result is available yet.'
      end
    end

    def close
      @mutex.synchronize { @worker.close }
    end
  end
end
