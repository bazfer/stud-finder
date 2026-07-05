# frozen_string_literal: true

require 'open3'

module StudFinder
  class Churn
    class Result
      attr_reader :churn_commits, :churn_lines, :zero_inflated, :zero_percentage

      def initialize(zero_inflated:, zero_percentage:, **values)
        @churn_commits = values[:churn_commits] || values[:counts] || {}
        @churn_lines = values[:churn_lines] || values[:line_counts] || {}
        @zero_inflated = zero_inflated
        @zero_percentage = zero_percentage
      end

      def counts
        churn_commits
      end

      def line_counts
        churn_lines
      end
    end

    class Error < StandardError; end

    def initialize(repo_path:, files:, days:, stderr: $stderr)
      @repo_path = File.expand_path(repo_path)
      @files = files
      @days = days
      @stderr = stderr
    end

    def call
      stdout, _stderr, status = git_log
      raise Error, "Error: #{@repo_path} is not a git repository." unless status.success?

      counts = initial_counts
      file_set = counts.keys.to_h { |file| [file, true] }
      line_counts = initial_counts
      stdout.each_line do |line|
        line = line.strip
        next if line.empty?

        added, deleted, path = line.split("\t", 3)
        next if path.nil?

        relative = normalize_path(renamed_path(path))
        next unless file_set[relative]

        counts[relative] += 1
        line_counts[relative] += added.to_i + deleted.to_i if added != '-' && numeric?(deleted)
      end

      Result.new(
        churn_commits: counts,
        churn_lines: line_counts,
        zero_inflated: zero_inflated?(counts),
        zero_percentage: zero_percentage(counts)
      ).tap { |result| warn_if_zero_inflated(result) }
    rescue Errno::ENOENT
      raise Error, 'Error: git not found in PATH.'
    end

    private

    def git_log
      Open3.capture3(
        'git', '-C', @repo_path, 'log',
        "--since=#{@days} days ago",
        '--format=tformat:',
        '--numstat',
        '--no-merges',
        '--find-renames',
        '--diff-filter=ACDMR'
      )
    end

    def initial_counts
      @files.to_h { |file| [file, 0] }
    end

    def normalize_path(path)
      absolute = File.expand_path(path, @repo_path)
      absolute.start_with?("#{@repo_path}/") ? absolute.delete_prefix("#{@repo_path}/") : path
    end

    def renamed_path(path)
      return path unless path.include?(' => ')

      path.sub(/\{[^{}]* => ([^{}]*)\}/, '\\1').then do |renamed|
        renamed == path ? path.split(' => ', 2).last : renamed
      end
    end

    def numeric?(value)
      value&.match?(/\A\d+\z/)
    end

    def zero_inflated?(counts)
      return false if counts.empty?

      counts.values.count(&:zero?) > counts.length * 0.5
    end

    def zero_percentage(counts)
      return 0 if counts.empty?

      ((counts.values.count(&:zero?).to_f / counts.length) * 100).round
    end

    def warn_if_zero_inflated(result)
      return unless result.zero_inflated

      @stderr.puts "Warning: #{result.zero_percentage}% of files have zero churn in the last #{@days} days. " \
                   'Churn signal is weak. Consider --churn-days to widen the window.'
    end
  end
end
