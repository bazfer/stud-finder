# frozen_string_literal: true

require 'open3'
require 'pathname'
require_relative 'directory_index'
require_relative 'source_snapshot'

module StudFinder
  class FileCollector
    DEFAULT_EXCLUDES = [
      '.git/**',
      '**/.git/**',
      'db/schema.rb',
      'db/migrate/**',
      'node_modules/**',
      '**/node_modules/**',
      'vendor/**',
      '**/*.min.js',
      'tmp/**',
      'log/**',
      'coverage/**',
      'spec/**',
      'test/**',
      '__tests__/**',
      '**/__tests__/**',
      '**/*.test.js',
      '**/*.test.ts',
      '**/*.test.jsx',
      '**/*.test.tsx',
      '**/*.spec.js',
      '**/*.spec.ts',
      '**/*.spec.jsx',
      '**/*.spec.tsx'
    ].freeze

    EXTENSIONS = %w[.rb .js .ts .jsx .tsx].freeze
    LANGUAGES = {
      '.rb' => :ruby,
      '.js' => :javascript,
      '.jsx' => :javascript,
      '.ts' => :typescript,
      '.tsx' => :typescript
    }.freeze

    FNM_FLAGS = File::FNM_PATHNAME | File::FNM_DOTMATCH

    Result = Struct.new(:files, :languages, :default_excluded_count, :custom_excluded_count, :pruned_directories,
                        :directories, :input_files,
                        keyword_init: true)

    class Error < StandardError; end

    def initialize(path:, excludes: [], min_files: 20, stderr: $stderr, prune: false, directory_index: nil)
      @path = File.expand_path(path)
      @excludes = excludes
      @min_files = min_files
      @stderr = stderr
      @prune = prune
      @directory_index = directory_index || DirectoryIndex.new
    end

    def collect
      validate!

      default_excluded = 0
      custom_excluded = 0
      languages = {}
      @pruned_directories = 0
      @directories = {}
      @input_files = []
      files = candidate_files
              .sort
              .filter_map do |file|
        extension = File.extname(file)
        next unless EXTENSIONS.include?(extension)

        relative = relative_path(file)

        if default_excluded?(relative, file)
          default_excluded += 1
          next
        end

        if excluded_by_patterns?(relative, @excludes)
          custom_excluded += 1
          next
        end

        languages[relative] = LANGUAGES.fetch(extension)
        relative
      end

      if files.length < 5
        raise Error,
              "Error: only #{files.length} supported files found after excludes. Too few for meaningful analysis."
      end

      if files.length < @min_files
        @stderr.puts "Warning: only #{files.length} files found. Percentile ranks are unreliable at this scale. " \
                     'Results are advisory only.'
      end

      Result.new(files: files, languages: languages, default_excluded_count: default_excluded,
                 custom_excluded_count: custom_excluded, pruned_directories: @pruned_directories,
                 directories: @directories, input_files: @input_files)
    end

    private

    def candidate_files
      return Dir.glob(File.join(@path, '**', '*'), File::FNM_DOTMATCH).select { |file| File.file?(file) } unless @prune

      # Only a trailing /** promises exclusion of the entire subtree. Keep
      # other patterns at the file filter so similarly named siblings survive.
      subtrees = (DEFAULT_EXCLUDES + @excludes).select { |pattern| pattern.end_with?('/**') }
      files = []
      walk_directory(@path, subtrees, files)
      @directory_index.retain(@directories)
      files
    end

    def walk_directory(directory, subtrees, files)
      stamp = SourceSnapshot.directory_stamp(directory)
      @directories[directory] = stamp
      @directory_index.entries(directory, stamp).each do |file, kind|
        if kind == :directory || (kind == :symlink && File.directory?(file))
          visit_directory_entry(file, kind, subtrees, files)
        elsif kind == :file || (kind == :symlink && File.file?(file))
          @input_files << file if file.match?(%r{\.(json|jsonc|ya?ml|cjs|mjs)\z|lock[^/]*\z})
          files << file if EXTENSIONS.include?(File.extname(file))
        end
      end
    end

    def visit_directory_entry(file, kind, subtrees, files)
      if excluded_by_patterns?("#{relative_path(file)}/__probe__", subtrees)
        @pruned_directories += 1
      elsif kind == :directory
        walk_directory(file, subtrees, files)
      else
        # Match Find.find: observe but never follow a nested directory symlink.
        @directories[file] = SourceSnapshot.directory_stamp(file)
      end
    end

    def validate!
      raise Error, "Error: #{@path} does not exist." unless File.exist?(@path)
      raise Error, "Error: #{@path} is not a directory." unless File.directory?(@path)
      raise Error, 'Error: git not found in PATH.' unless git_available?

      _stdout, _stderr, status = Open3.capture3('git', '-C', @path, 'rev-parse', '--is-inside-work-tree')
      return if status.success?

      raise Error, "Error: #{@path} is not a git repository."
    end

    def git_available?
      ENV.fetch('PATH', '').split(File::PATH_SEPARATOR).any? do |dir|
        git = File.join(dir, 'git')
        File.file?(git) && File.executable?(git)
      end
    end

    def relative_path(file)
      file.delete_prefix("#{@path}/")
    end

    def default_excluded?(relative, file)
      excluded_by_patterns?(relative, DEFAULT_EXCLUDES) || auto_generated?(file)
    end

    def excluded_by_patterns?(relative, patterns)
      patterns.any? { |pattern| glob_match?(pattern, relative) }
    end

    def glob_match?(pattern, relative)
      File.fnmatch(pattern, relative, FNM_FLAGS) ||
        globstar_directory_match?(pattern, relative) ||
        (pattern.end_with?('/**') && relative.start_with?("#{pattern.delete_suffix('/**')}/"))
    end

    def globstar_directory_match?(pattern, relative)
      return false unless pattern.start_with?('**/') && pattern.end_with?('/**')

      directory = pattern.delete_prefix('**/').delete_suffix('/**')
      relative.start_with?("#{directory}/") || relative.include?("/#{directory}/")
    end

    def auto_generated?(file)
      File.foreach(file) do |line|
        next if line.strip.empty?

        return line.match?(/\A\s*#\s*This file is auto-generated/i)
      end
      false
    rescue Encoding::InvalidByteSequenceError, Encoding::UndefinedConversionError, ArgumentError
      false
    end
  end
end
