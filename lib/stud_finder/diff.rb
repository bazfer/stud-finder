# frozen_string_literal: true

require 'open3'

module StudFinder
  # Resolves the set of files changed on HEAD relative to the merge-base with a
  # base ref (e.g. origin/staging), as repo-root-relative paths that match the
  # form FileCollector emits. Used to filter output down to a PR's touched files
  # WITHOUT narrowing the analysis population — scoring still runs against the
  # full repo, so fan_in and percentiles stay correct.
  class Diff
    class Error < StandardError; end

    def initialize(repo_path:, base_ref:)
      @repo_path = File.expand_path(repo_path)
      @base_ref = base_ref
    end

    def validate_ref!
      verify_ref!
    end

    def changed_paths
      verify_ref!
      stdout, stderr, status = Open3.capture3(
        'git', '-C', @repo_path, 'diff', '--name-only', '--diff-filter=d', "#{@base_ref}...HEAD"
      )
      raise Error, diff_error(stderr) unless status.success?

      stdout.each_line.map(&:strip).reject(&:empty?)
    rescue Errno::ENOENT
      raise Error, 'Error: git not found in PATH.'
    end

    private

    def verify_ref!
      _stdout, _stderr, status = Open3.capture3(
        'git', '-C', @repo_path, 'rev-parse', '--verify', '--quiet', "#{@base_ref}^{commit}"
      )
      return if status.success?

      raise Error, "Error: diff base ref not found: #{@base_ref}"
    end

    def diff_error(stderr)
      message = stderr.to_s.strip
      return 'Error: git diff failed.' if message.empty?

      "Error: git diff failed: #{message}"
    end
  end
end
