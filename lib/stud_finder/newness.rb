# frozen_string_literal: true

require 'open3'
require 'pathname'
require 'set'

module StudFinder
  # rubocop:disable Metrics/ClassLength
  class Newness
    DEFAULT_DAYS = 30
    DEFAULT_MIN_COMMITS = 3
    SECONDS_PER_DAY = 86_400
    SHALLOW_CLONE_WARNING = {
      code: 'shallow_clone_newness_disabled',
      message: 'shallow git clone detected; newness rules disabled (use fetch-depth: 0 in CI for full newness behavior)'
    }.freeze

    History = Struct.new(:first_commit_epoch, :total_commits, keyword_init: true)

    class Error < StandardError; end

    def initialize(repo_path:, files:, days: DEFAULT_DAYS, min_commits: DEFAULT_MIN_COMMITS, enabled: true,
                   now: Time.now)
      @repo_path = File.expand_path(repo_path)
      @files = files
      @days = days
      @min_commits = min_commits
      @enabled = enabled
      @now = now.to_i
    end

    def call
      return self.class.disabled_metadata(@files) unless @enabled
      return self.class.disabled_metadata(@files) if self.class.shallow_repository?(@repo_path)

      history = git_history
      @files.to_h do |file|
        first_epoch = history.first_commit_epoch[file]
        total_commits = history.total_commits.fetch(file, 0)
        age_days = age_days(first_epoch)
        is_new = new_file?(age_days, total_commits)

        [file, { new_file: is_new, age_days: age_days || 0, total_commits: total_commits, escalation: '' }]
      end
    end

    def self.apply(rows:, edges:, metadata:, branch_threshold: 'branch')
      rows = rows.map { |row| row.merge(newness_fields(metadata.fetch(row[:path], nil))) }
      trunk_paths = rows.select { |row| row[:classification] == 'trunk' }.to_set { |row| row[:path] }

      rows.map do |row|
        next row unless row[:new_file]

        dependencies = edges.fetch(row[:path], {}).fetch(:dependencies, [])
        if dependencies.any? { |path| trunk_paths.include?(path) }
          row.merge(classification: 'trunk', escalation: 'trunk_adjacent')
        elsif row[:classification] == 'leaf'
          row.merge(classification: branch_threshold, escalation: 'recency_floor')
        else
          row
        end
      end
    end

    def self.disabled_metadata(files)
      files.to_h { |file| [file, { new_file: false, age_days: 0, total_commits: 0, escalation: '' }] }
    end

    def self.shallow_repository?(repo_path)
      repo_path = File.expand_path(repo_path)
      stdout, _stderr, status = Open3.capture3('git', 'rev-parse', '--is-shallow-repository', chdir: repo_path)
      return stdout.strip == 'true' if status.success?

      git_shallow_file?(repo_path)
    rescue Errno::ENOENT
      git_shallow_file?(repo_path)
    end

    def self.newness_fields(metadata)
      metadata ||= { new_file: false, age_days: 0 }
      {
        new_file: metadata.fetch(:new_file, false),
        age_days: metadata.fetch(:age_days, 0),
        escalation: ''
      }
    end

    def self.git_shallow_file?(repo_path)
      return true if File.exist?(File.join(repo_path, '.git', 'shallow'))

      stdout, _stderr, status = Open3.capture3('git', 'rev-parse', '--git-dir', chdir: repo_path)
      status.success? && File.exist?(File.join(File.expand_path(stdout.strip, repo_path), 'shallow'))
    rescue Errno::ENOENT
      false
    end
    private_class_method :git_shallow_file?

    private

    def empty_history
      History.new(first_commit_epoch: {}, total_commits: {})
    end

    def git_history
      stdout, _stderr, status = Open3.capture3(
        'git', '-C', @repo_path, 'log',
        '--format=%x1e%at',
        '--name-status',
        '--find-renames',
        '--diff-filter=ACMRD'
      )
      raise Error, "Error: #{@repo_path} is not a git repository." unless status.success?

      parse_history(stdout)
    rescue Errno::ENOENT
      raise Error, 'Error: git not found in PATH.'
    end

    # rubocop:disable Metrics/AbcSize, Metrics/CyclomaticComplexity, Metrics/PerceivedComplexity
    def parse_history(stdout)
      wanted = @files.to_set
      first_commit_epoch = {}
      total_commits = Hash.new(0)
      aliases = {}
      current_epoch = nil
      touched = Set.new
      lineage_boundaries = Set.new

      flush_commit = lambda do
        touched.each do |file|
          total_commits[file] += 1
          first_commit_epoch[file] = [first_commit_epoch[file], current_epoch].compact.min if current_epoch
        end
        touched.clear
      end

      stdout.each_line(chomp: true) do |line|
        if line.start_with?("\x1e")
          flush_commit.call if current_epoch
          current_epoch = line.delete_prefix("\x1e").to_i
          next
        end

        deleted_path = deleted_path_for_status(line)
        if deleted_path
          canonical = trackable_canonical_path(deleted_path, aliases)
          lineage_boundaries << deleted_path if canonical && wanted.include?(canonical)
          next
        end

        paths_for_status(line).each do |path|
          canonical = trackable_canonical_path(path, aliases)
          next unless canonical && wanted.include?(canonical)
          next if lineage_boundaries.include?(path)

          touched << canonical
        end

        old_path, new_path = rename_paths_for_status(line)
        next unless old_path && new_path

        new_rebased = rebase_to_analysis_root(new_path)
        next unless new_rebased

        canonical_new = canonical_path(new_rebased, aliases)
        if wanted.include?(canonical_new) && !lineage_boundaries.include?(old_path)
          touched << canonical_new
          aliases[alias_key_for(old_path)] = canonical_new
        end
      end
      flush_commit.call if current_epoch

      History.new(first_commit_epoch: first_commit_epoch, total_commits: total_commits)
    end

    # rubocop:enable Metrics/AbcSize, Metrics/CyclomaticComplexity, Metrics/PerceivedComplexity

    def paths_for_status(line)
      return [] if line.nil? || line.empty?

      parts = line.split("\t")
      status = parts.first.to_s
      if status.start_with?('R') || status.start_with?('C')
        [parts[2]].compact
      else
        [parts[1]].compact
      end
    end

    def rename_paths_for_status(line)
      return [nil, nil] if line.nil? || line.empty?

      parts = line.split("\t")
      return [nil, nil] unless parts.first.to_s.start_with?('R')

      [parts[1], parts[2]]
    end

    def deleted_path_for_status(line)
      return nil if line.nil? || line.empty?

      parts = line.split("\t")
      return nil unless parts.first == 'D'

      parts[1]
    end

    def trackable_canonical_path(raw_path, aliases)
      rebased = rebase_to_analysis_root(raw_path)
      return nil unless !rebased.nil? || aliases.key?(raw_path)

      canonical_path(rebased || raw_path, aliases)
    end

    def alias_key_for(old_path)
      rebase_to_analysis_root(old_path) || old_path
    end

    def canonical_path(path, aliases)
      canonical = path
      seen = Set.new
      while aliases.key?(canonical) && !seen.include?(canonical)
        seen << canonical
        canonical = aliases[canonical]
      end
      canonical
    end

    # git log emits paths relative to the repository root even when -C points at
    # an analysis subdirectory. Rebase those records to the analysis root so they
    # can be compared with FileCollector paths (for example, app/models/user.rb
    # becomes models/user.rb when scanning <repo>/app).
    def rebase_to_analysis_root(path)
      return nil if path.nil? || path.empty?
      return path unless analysis_root_prefix

      path.start_with?(analysis_root_prefix) ? path.delete_prefix(analysis_root_prefix) : nil
    end

    def analysis_root_prefix
      return @analysis_root_prefix if defined?(@analysis_root_prefix)

      @analysis_root_prefix = begin
        toplevel = git_toplevel
        analysis_abs = File.realpath(@repo_path)
        if toplevel.nil? || analysis_abs == toplevel
          nil
        else
          prefix = Pathname.new(analysis_abs).relative_path_from(Pathname.new(toplevel)).to_s
          prefix.empty? || prefix == '.' || prefix.start_with?('..') ? nil : "#{prefix}/"
        end
      end
    rescue Errno::ENOENT
      nil
    end

    def git_toplevel
      return @git_toplevel if defined?(@git_toplevel)

      stdout, _stderr, status = Open3.capture3('git', '-C', @repo_path, 'rev-parse', '--show-toplevel')
      @git_toplevel = status.success? ? File.realpath(stdout.strip) : nil
    end

    def age_days(first_epoch)
      return nil unless first_epoch

      [((@now - first_epoch) / SECONDS_PER_DAY).floor, 0].max
    end

    def new_file?(age_days, total_commits)
      within_age_window = @days.positive? && !age_days.nil? && age_days < @days
      low_commit_count = @min_commits.positive? && total_commits < @min_commits
      unknown_lineage = age_days.nil?

      unknown_lineage || within_age_window || low_commit_count
    end
  end
  # rubocop:enable Metrics/ClassLength
end
