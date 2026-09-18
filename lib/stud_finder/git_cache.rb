# frozen_string_literal: true

require 'open3'
require_relative 'fact_cache'

module StudFinder
  # One scan's shared Git reader. Persist raw successful log output, never final
  # scores or ages. HEAD, effective config, replacement refs and shallow/graft
  # boundaries identify history; timed logs expire at the next commit boundary.
  class GitCache
    Status = Struct.new(:exitstatus) do
      def success?
        exitstatus.zero?
      end
    end

    attr_reader :head

    def initialize(repo_path:, directory:, stderr: $stderr, now: Time.now)
      @repo_path = File.realpath(repo_path)
      @now = now.to_i
      @memory = {}
      @cache = FactCache.new(directory: directory, namespace: "git-v1:#{@repo_path}", stderr: stderr)
      @identity = history_identity
    end

    def capture3(*args)
      return Open3.capture3(*args) unless @identity
      return @memory[args] if @memory.key?(args)

      key = JSON.generate(args)
      value = @cache.fetch(key, @identity)
      return @memory[args] = [value['stdout'], '', Status.new(0)] if valid_value?(value)

      result = Open3.capture3(*args)
      if result.last.success?
        @cache.store(key, @identity,
                     { 'stdout' => result.first, 'expires_at' => expires_at(args), 'created_at' => @now })
        @cache.save
      end
      @memory[args] = result
    end

    # Derived history facts share the same validated Git identity as raw logs.
    # Callers include their implementation/options in the key and validate shape.
    def derive(key, valid:)
      return yield unless @identity

      cache_key = "derived:#{JSON.generate(key)}"
      value = @cache.fetch(cache_key, @identity)
      return value if valid.call(value)

      value = yield
      @cache.store(cache_key, @identity, value)
      @cache.save
      value
    end

    private

    def valid_value?(value)
      value.is_a?(Hash) && value['stdout'].is_a?(String) &&
        value['created_at'].is_a?(Integer) && @now >= value['created_at'] &&
        (value['expires_at'].nil? || (value['expires_at'].is_a?(Integer) && @now < value['expires_at']))
    end

    def history_identity
      refs, _, status = Open3.capture3('git', '-C', @repo_path, 'rev-parse', 'HEAD', '--absolute-git-dir',
                                       '--git-common-dir')
      return unless status.success?

      config, _, config_status = Open3.capture3('git', '-C', @repo_path, 'config', '--null', '--list', '--show-origin')
      replacements, _, refs_status = Open3.capture3('git', '-C', @repo_path, 'for-each-ref',
                                                    '--format=%(refname):%(objectname)',
                                                    'refs/replace')
      return unless config_status.success? && refs_status.success?

      @head = refs.lines.first.strip
      paths = refs.lines.drop(1).map { |path| File.expand_path(path.strip, @repo_path) }.uniq
      boundaries = paths.flat_map do |path|
        %w[shallow info/grafts].map do |name|
          file = File.join(path, name)
          File.file?(file) ? File.binread(file) : ''
        end
      end
      environment = ENV.select { |key, _| key.start_with?('GIT_') || %w[TZ LC_ALL LC_CTYPE LANG].include?(key) }
      Digest::SHA256.hexdigest(JSON.generate([refs, config, replacements, boundaries, environment.sort]))
    rescue SystemCallError
      nil
    end

    def expires_at(args)
      since = args.find { |arg| arg.start_with?('--since=') }
      return unless since

      days = since[/\A--since=(\d+) days ago\z/, 1]
      return @now unless days

      # Cache the complete timestamp list by history identity, so unchanged
      # scans need no history traversal merely to validate a sliding window.
      times = @cache.fetch('commit-times', @identity)
      unless times.is_a?(Array) && times.all?(Integer)
        output, _, status = Open3.capture3('git', '-C', @repo_path, 'log', '--format=%ct', '--no-patch')
        return @now unless status.success?

        times = output.lines.map(&:to_i)
        @cache.store('commit-times', @identity, times)
      end
      times.map { |time| time + (days.to_i * 86_400) + 1 }.select { |time| time > @now }.min
    end
  end
end
