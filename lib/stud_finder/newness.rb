# frozen_string_literal: true

require 'open3'
require 'pathname'
require 'set'

module StudFinder
  class Newness
    DEFAULT_DAYS = 30
    DEFAULT_MIN_COMMITS = 3
    SECONDS_PER_DAY = 86_400

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
      history = @enabled ? git_history : empty_history
      @files.to_h do |file|
        first_epoch = history.first_commit_epoch[file]
        total_commits = history.total_commits.fetch(file, 0)
        age_days = age_days(first_epoch)
        is_new = @enabled && new_file?(age_days, total_commits)

        [file, { new_file: is_new, age_days: age_days || 0, total_commits: total_commits }]
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
      files.to_h { |file| [file, { new_file: false, age_days: 0, total_commits: 0 }] }
    end

    def self.newness_fields(metadata)
      metadata ||= { new_file: false, age_days: 0 }
      {
        new_file: metadata.fetch(:new_file, false),
        age_days: metadata.fetch(:age_days, 0),
        escalation: ''
      }
    end

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
        '--diff-filter=ACMR'
      )
      raise Error, "Error: #{@repo_path} is not a git repository." unless status.success?

      parse_history(stdout)
    rescue Errno::ENOENT
      raise Error, 'Error: git not found in PATH.'
    end

    def parse_history(stdout)
      wanted = @files.to_set
      first_commit_epoch = {}
      total_commits = Hash.new(0)
      aliases = {}
      current_epoch = nil
      touched = Set.new

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

        paths_for_status(line).each do |path|
          canonical = canonical_path(path, aliases)
          touched << canonical if wanted.include?(canonical)
        end

        old_path, new_path = rename_paths_for_status(line)
        next unless old_path && new_path

        canonical_new = canonical_path(new_path, aliases)
        if wanted.include?(canonical_new)
          touched << canonical_new
          aliases[old_path] = canonical_new
        end
      end
      flush_commit.call if current_epoch

      History.new(first_commit_epoch: first_commit_epoch, total_commits: total_commits)
    end

    def paths_for_status(line)
      return [] if line.nil? || line.empty?

      parts = line.split("\t")
      status = parts.first.to_s
      if status.start_with?('R') || status.start_with?('C')
        [rebase_to_analysis_root(parts[2])].compact
      else
        [rebase_to_analysis_root(parts[1])].compact
      end
    end

    def rename_paths_for_status(line)
      return [nil, nil] if line.nil? || line.empty?

      parts = line.split("\t")
      return [nil, nil] unless parts.first.to_s.start_with?('R')

      [rebase_to_analysis_root(parts[1]), rebase_to_analysis_root(parts[2])]
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
end
