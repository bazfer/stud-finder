# frozen_string_literal: true

require 'json'
require 'open3'
require 'timeout'

module StudFinder
  # A session-owned Node process. JSON-lines over pipes, no network listener and
  # no global cwd changes in the Ruby host. Requests are serialized by Session.
  class JsWorker
    class Error < StandardError; end

    def initialize
      @root = nil
    end

    def call(root:, operation:, files:, timeout:, cache_dir:, config: nil)
      root = File.realpath(root)
      start(root) unless @root == root && @wait&.alive?
      Timeout.timeout(timeout) do
        @input.puts(JSON.generate(operation: operation, files: files, cacheDir: cache_dir, configPath: config))
        @input.flush
        line = @output.gets
        raise Error, 'JavaScript worker exited' unless line

        response = JSON.parse(line)
        raise Error, response['error'] if response['error']

        response.fetch('result')
      end
    rescue Timeout::Error, IOError, Errno::EPIPE, Errno::ENOENT, JSON::ParserError => e
      close
      raise Error, "JavaScript worker failed: #{e.class}"
    end

    def close
      @input&.close unless @input&.closed?
      if @wait&.alive?
        Process.kill('TERM', @wait.pid)
        Process.kill('KILL', @wait.pid) unless @wait.join(1)
      end
      @wait&.join
      [@output, @errors].compact.each { |io| io.close unless io.closed? }
      @root = nil
    rescue Errno::ESRCH, IOError
      @root = nil
    end

    private

    def start(root)
      close
      @input, @output, @errors, @wait = Open3.popen3('node', File.join(__dir__, 'js_worker.mjs'), chdir: root)
      @root = root
      # Drain diagnostics so a noisy dependency cannot block the JSON protocol.
      @drainer = Thread.new { @errors.each_line { |_line| nil } }
      @drainer.report_on_exception = false
    end
  end
end
