# frozen_string_literal: true

require 'open3'

module StudFinder
  class TemporalCoupling
    Result = Struct.new(:pairs, :warnings, keyword_init: true)

    SHA_PATTERN = /\A[0-9a-f]{40}\z/

    def initialize(repo_path:, files:, days:, min_co_changes: 5, coupling_threshold: 0.30, max_commit_files: 50)
      @repo_path = File.expand_path(repo_path)
      @file_set = files.to_h { |f| [f, true] }
      @days = days
      @min_co_changes = min_co_changes
      @coupling_threshold = coupling_threshold
      @max_commit_files = max_commit_files
    end

    def call
      stdout, _err, status = git_log
      return Result.new(pairs: {}, warnings: ['git_error']) unless status.success?

      commits, skipped_bulk_commits = parse_commits(stdout)
      co_matrix = build_co_change_matrix(commits)
      own_changes = build_own_changes(commits)
      Result.new(pairs: build_pairs(co_matrix, own_changes), warnings: warnings(skipped_bulk_commits))
    rescue Errno::ENOENT
      Result.new(pairs: {}, warnings: ['git_not_found'])
    end

    private

    def git_log
      Open3.capture3(
        'git', '-C', @repo_path, 'log',
        "--since=#{@days} days ago",
        '--diff-filter=ACDMR',
        '--name-only',
        '--relative',
        '--format=%H'
      )
    end

    def parse_commits(stdout)
      commits = []
      current = nil
      skipped_bulk_commits = 0
      stdout.each_line do |raw|
        line = raw.chomp
        if SHA_PATTERN.match?(line)
          skipped_bulk_commits += append_commit(commits, current)
          current = []
        elsif !line.empty? && current
          relative = normalize_path(line)
          # Guard against the same path appearing twice in one commit's --name-only
          # output: a dup would inflate own_changes and create a spurious self-pair.
          current << relative if @file_set[relative] && !current.include?(relative)
        end
      end
      skipped_bulk_commits += append_commit(commits, current)
      [commits, skipped_bulk_commits]
    end

    def append_commit(commits, files)
      return 0 unless files&.any?

      return 1 if @max_commit_files.positive? && files.length > @max_commit_files

      commits << files
      0
    end

    def build_co_change_matrix(commits)
      matrix = Hash.new { |h, k| h[k] = Hash.new(0) }
      commits.each do |files|
        files.combination(2).each do |a, b|
          a, b = b, a if a > b
          matrix[a][b] += 1
        end
      end
      matrix
    end

    def build_own_changes(commits)
      counts = Hash.new(0)
      commits.each { |files| files.each { |f| counts[f] += 1 } }
      counts
    end

    def build_pairs(co_matrix, own_changes)
      pairs = Hash.new { |h, k| h[k] = [] }
      co_matrix.each do |a, partners|
        partners.each do |b, count|
          next if count < @min_co_changes

          min_own = [own_changes[a], own_changes[b]].min
          next if min_own.zero?

          coupling = (count.to_f / min_own).round(4)
          next if coupling < @coupling_threshold

          pairs[a] << { path: b, coupling: coupling, co_changes: count, own_changes: own_changes[b] }
          pairs[b] << { path: a, coupling: coupling, co_changes: count, own_changes: own_changes[a] }
        end
      end
      pairs.transform_values { |p| p.sort_by { |e| -e[:coupling] } }
    end

    def normalize_path(path)
      path = renamed_path(path)
      absolute = File.expand_path(path, @repo_path)
      absolute.start_with?("#{@repo_path}/") ? absolute.delete_prefix("#{@repo_path}/") : path
    end

    def renamed_path(path)
      return path unless path.include?(' => ')

      path.sub(/\{[^{}]* => ([^{}]*)\}/, '\\1').then do |renamed|
        renamed == path ? path.split(' => ', 2).last : renamed
      end
    end

    def warnings(skipped_bulk_commits)
      return [] if skipped_bulk_commits.zero?

      [{ code: 'temporal_coupling_bulk_commits_skipped', count: skipped_bulk_commits,
         max_commit_files: @max_commit_files }]
    end
  end
end
