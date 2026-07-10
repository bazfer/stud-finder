# frozen_string_literal: true

require 'set'

module StudFinder
  module Coverage
    class Lcov
      class Error < StandardError; end

      attr_reader :missing_files

      def initialize(path:, files:, project_root: nil)
        @path = path
        @files = files
        @file_set = Set.new(files)
        @project_root = File.expand_path(project_root) if project_root
        @missing_files = []
      end

      def call
        reported = parse_report
        @missing_files = @files.reject { |file| reported.key?(file) }
        reported.slice(*@files)
      end

      private

      def parse_report
        records = File.read(@path).split(/^end_of_record\s*$/)
        records.each_with_object({}) do |record, coverage|
          filename = record[/^SF:(.+)$/, 1]
          next if filename.nil? || filename.empty?

          coverage[normalize_filename(filename)] = line_rate(record, filename)
        end
      rescue Errno::ENOENT
        raise Error, "Error: coverage file not found: #{@path}"
      end

      def normalize_filename(filename)
        expanded = File.expand_path(filename)
        stripped = project_root_stripped(filename, expanded)
        return stripped if stripped && @file_set.include?(stripped)

        if filename.start_with?('/')
          suffix_match(filename) || stripped || filename.delete_prefix('./')
        else
          stripped || filename.delete_prefix('./')
        end
      end

      def project_root_stripped(filename, expanded)
        if @project_root && filename.start_with?("#{@project_root}/")
          filename.delete_prefix("#{@project_root}/")
        elsif @project_root && expanded.start_with?("#{@project_root}/")
          expanded.delete_prefix("#{@project_root}/")
        end
      end

      def suffix_match(filename)
        components = filename.split('/').reject(&:empty?)

        components.length.downto(1) do |count|
          suffix = components.last(count).join('/')
          return suffix if @file_set.include?(suffix)
        end

        nil
      end

      def line_rate(record, filename)
        found = integer_field(record, 'LF')
        hit = integer_field(record, 'LH')

        if found.nil? || hit.nil?
          lines = record.scan(/^DA:\d+,(\d+)/).flatten.map(&:to_i)
          found = lines.length
          hit = lines.count(&:positive?)
        end

        return 0.0 if found.zero?
        raise Error, "Error: line hits exceed lines found for coverage file: #{filename}" if hit > found

        hit.to_f / found
      end

      def integer_field(record, field)
        raw = record[/^#{field}:(\d+)$/, 1]
        raw&.to_i
      end
    end
  end
end
