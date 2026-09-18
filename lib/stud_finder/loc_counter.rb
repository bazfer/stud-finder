# frozen_string_literal: true

require_relative 'fact_cache'

module StudFinder
  class LocCounter
    def initialize(repo_path:, files:, cache_dir: nil)
      @repo_path = File.expand_path(repo_path)
      @files = files
      @cache_dir = cache_dir
    end

    def call
      return @files.to_h { |file| [file, non_blank_lines(file)] } unless @cache_dir

      cache = FactCache.new(directory: @cache_dir, namespace: "loc:#{Digest::SHA256.file(__FILE__).hexdigest}")
      result = @files.to_h do |file|
        fingerprint = Digest::SHA256.file(File.join(@repo_path, file)).hexdigest
        count = cache.fetch(file, fingerprint)
        unless count.is_a?(Integer) && count >= 0
          count = non_blank_lines(file)
          cache.store(file, fingerprint, count)
        end
        [file, count]
      end
      cache.save
      result
    end

    private

    def non_blank_lines(file)
      File.foreach(File.join(@repo_path, file)).count { |line| !line.strip.empty? }
    rescue Encoding::InvalidByteSequenceError, Encoding::UndefinedConversionError, ArgumentError
      0
    end
  end
end
