# frozen_string_literal: true

require 'json'
require 'set'

module StudFinder
  module Coverage
    class Resultset
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
        data = JSON.parse(File.read(@path))
        merged = {}

        coverage_payloads(data).each do |coverage|
          coverage.each do |filename, details|
            lines = details.is_a?(Hash) ? details['lines'] : details
            next unless lines.is_a?(Array)

            key = normalize_filename(filename)
            merged[key] = merged.key?(key) ? merge_lines(merged[key], lines) : lines
          end
        end

        merged.transform_values { |lines| line_rate(lines) }
      rescue JSON::ParserError => e
        raise Error, "Error: malformed coverage JSON: #{e.message.lines.first.strip}"
      rescue Errno::ENOENT
        raise Error, "Error: coverage file not found: #{@path}"
      end

      def merge_lines(previous_lines, new_lines)
        max_length = [previous_lines.length, new_lines.length].max

        (0...max_length).map do |index|
          previous = previous_lines[index]
          current = new_lines[index]

          previous.nil? && current.nil? ? nil : [previous || 0, current || 0].max
        end
      end

      def coverage_payloads(data)
        if data['coverage'].is_a?(Hash)
          [data['coverage']]
        else
          data.values.filter_map { |suite| suite['coverage'] if suite.is_a?(Hash) }
        end
      end

      def normalize_filename(filename)
        stripped = project_root_stripped(filename)
        return stripped if stripped && @file_set.include?(stripped)

        if filename.start_with?('/')
          suffix_match(filename) || stripped || filename.delete_prefix('./')
        else
          stripped || filename.delete_prefix('./')
        end
      end

      def project_root_stripped(filename)
        filename.delete_prefix("#{@project_root}/") if @project_root && filename.start_with?("#{@project_root}/")
      end

      def suffix_match(filename)
        components = filename.split('/').reject(&:empty?)

        components.length.downto(1) do |count|
          suffix = components.last(count).join('/')
          return suffix if @file_set.include?(suffix)
        end

        nil
      end

      def line_rate(lines)
        executable = lines.compact
        return 0.0 if executable.empty?

        executable.count(&:positive?).to_f / executable.length
      end
    end
  end
end
