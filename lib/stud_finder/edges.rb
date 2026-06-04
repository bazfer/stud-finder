# frozen_string_literal: true

module StudFinder
  class Edges
    MAX_ROWS = 50

    def initialize(target:, rows:, edges:, stdout: $stdout, stderr: $stderr)
      @target = target
      @rows = rows.to_h { |row| [row[:path], row] }
      @edges = edges
      @stdout = stdout
      @stderr = stderr
    end

    def call
      if @target.nil? || @target.empty?
        @stderr.puts 'Usage: stud-finder edges FILE [PATH]'
        return 1
      end

      unless @edges.key?(@target)
        @stderr.puts "Error: '#{@target}' was not found in the scored file set."
        return 1
      end

      target_row = @rows[@target]
      edge_data = @edges[@target]

      emit_header(target_row)
      emit_section('Dependents', edge_data[:dependents], '(files that depend on this file — blast radius)')
      emit_section('Dependencies', edge_data[:dependencies], '(files this file depends on — fan-out)')
      @stdout.puts
      @stdout.puts 'Edges are statically computed — dynamic references not counted.'
      0
    end

    private

    def emit_header(row)
      @stdout.puts
      @stdout.puts "stud-finder edges — #{@target}"
      @stdout.puts
      if row
        @stdout.puts format('  score: %<score>s  class: %<class>-6s  fan_in: %<fan_in>d  ' \
                            'fan_out: %<fan_out>d  instability: %<instability>s',
                            score: format_score(row[:score]), class: row[:classification],
                            fan_in: row[:fan_in], fan_out: row[:fan_out],
                            instability: format_score(row[:instability]))
      end
      @stdout.puts
    end

    def emit_section(title, paths, description)
      scored, unscored = paths.partition { |p| @rows.key?(p) }
      scored_rows = scored.map { |p| @rows[p] }.sort_by { |r| -r[:score] }.first(MAX_ROWS)

      @stdout.puts "  ── #{title} (#{paths.length} files) #{description} ──"
      @stdout.puts

      if scored_rows.empty?
        @stdout.puts '    (none in scored file set)'
      else
        @stdout.puts format('  %<rank>4s  %<path>-50s  %<score>6s  %<class>-6s  %<fan_in>6s  %<fan_out>6s  ' \
                            '%<instability>11s',
                            rank: 'rank', path: 'file', score: 'score', class: 'class',
                            fan_in: 'fan_in', fan_out: 'fan_out', instability: 'instability')
        scored_rows.each_with_index do |row, i|
          @stdout.puts format('  %<rank>4d  %<path>-50s  %<score>6s  %<class>-6s  %<fan_in>6d  %<fan_out>6d  ' \
                              '%<instability>11s',
                              rank: i + 1, path: row[:path], score: format_score(row[:score]),
                              class: row[:classification], fan_in: row[:fan_in], fan_out: row[:fan_out],
                              instability: format_score(row[:instability]))
        end
        @stdout.puts "    ... and #{unscored.length} unscored (gems, generated files)" if unscored.any?
      end
      @stdout.puts
    end

    def format_score(score)
      format('%.4f', score)
    end
  end
end
