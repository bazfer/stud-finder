# frozen_string_literal: true

module StudFinder
  class LocCounter
    def initialize(repo_path:, files:)
      @repo_path = File.expand_path(repo_path)
      @files = files
    end

    def call
      @files.to_h { |file| [file, non_blank_lines(file)] }
    end

    private

    def non_blank_lines(file)
      File.foreach(File.join(@repo_path, file)).count { |line| !line.strip.empty? }
    rescue Encoding::InvalidByteSequenceError, Encoding::UndefinedConversionError, ArgumentError
      0
    end
  end
end
