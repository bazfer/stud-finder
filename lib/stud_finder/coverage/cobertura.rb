# frozen_string_literal: true

require 'rexml/document'

module StudFinder
  module Coverage
    class Cobertura
      class Error < StandardError; end

      attr_reader :missing_files

      def initialize(path:, files:, project_root: nil)
        @path = path
        @files = files
        @project_root = project_root
        @missing_files = []
      end

      def call
        reported = parse_report
        @missing_files = @files.reject { |file| reported.key?(file) }
        reported.slice(*@files)
      end

      private

      def parse_report
        document = REXML::Document.new(File.read(@path))
        {}.tap do |coverage|
          REXML::XPath.each(document, '/coverage/packages/package/classes/class') do |element|
            filename = element.attributes['filename']
            next if filename.nil? || filename.empty?

            coverage[normalize_filename(filename)] = parse_line_rate(filename, element.attributes['line-rate'])
          end
        end
      rescue REXML::ParseException => e
        raise Error, "Error: malformed coverage XML: #{e.message.lines.first.strip}"
      rescue Errno::ENOENT
        raise Error, "Error: coverage file not found: #{@path}"
      end

      def normalize_filename(filename)
        filename.delete_prefix('./')
      end

      def parse_line_rate(filename, raw_rate)
        raise Error, "Error: missing line-rate for coverage file: #{filename}" if raw_rate.nil? || raw_rate.empty?

        rate = Float(raw_rate)
        return rate if rate.between?(0.0, 1.0)

        raise Error, "Error: line-rate out of range for coverage file: #{filename}"
      rescue ArgumentError
        raise Error, "Error: invalid line-rate for coverage file: #{filename}"
      end
    end
  end
end
