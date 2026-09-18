# frozen_string_literal: true

require 'digest'
require 'json'
require 'open3'
require_relative 'complexity'

module StudFinder
  # Detect edits during a scan instead of publishing mixed source generations.
  # Directory stamps detect additions/deletions; content hashes detect edits
  # even when file size and mtime were preserved by the editor or checkout.
  class SourceSnapshot
    class Changed < StandardError; end

    attr_reader :generation

    def initialize(root:, files:, directories:, extra_files: [], cache_dir: nil, head: nil)
      @root = root
      extra_files = extra_files.compact.reject do |file|
        cache_dir && File.expand_path(file).start_with?("#{cache_dir}/")
      end
      @files = (files.map { |file| File.join(root, file) } + extra_files).uniq.sort
      @directories = directories.reject do |path, _stamp|
        cache_dir && (path == cache_dir || path.start_with?("#{cache_dir}/"))
      end
      @before = contents
      @head = head || self.head
      @ruby_version = Complexity.target_ruby_version(@root)
      @generation = Digest::SHA256.hexdigest(JSON.generate([@before, @head, @ruby_version]))
    end

    def verify!
      return if @before == contents && @head == head && @ruby_version == Complexity.target_ruby_version(@root) &&
                @directories.all? { |path, stamp| stamp == self.class.directory_stamp(path) }

      raise Changed, 'Files or HEAD changed during analysis; retry the scan.'
    rescue SystemCallError
      raise Changed, 'Files changed during analysis; retry the scan.'
    end

    def self.directory_stamp(path)
      stat = File.stat(path)
      [stat.ino, stat.mtime.to_i, stat.mtime.nsec, stat.ctime.to_i, stat.ctime.nsec]
    end

    private

    def contents
      @files.map do |file|
        next [file, nil] unless File.file?(file)

        digest = Digest::SHA256.file(file).hexdigest
        stat = File.stat(file)
        [file, digest, stat.ino, stat.mtime.to_i, stat.mtime.nsec, stat.ctime.to_i, stat.ctime.nsec]
      end
    end

    def head
      output, _, status = Open3.capture3('git', '-C', @root, 'rev-parse', 'HEAD')
      status.success? ? output.strip : nil
    end
  end
end
