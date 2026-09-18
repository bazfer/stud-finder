# frozen_string_literal: true

require 'json'
require 'stringio'
require_relative '../lib/stud_finder/session'

abort 'Usage: bundle exec ruby benchmark/complete_pipeline.rb REPO CACHE_DIR [SCAN_OPTIONS...]' if ARGV.length < 2
repo, cache = ARGV.shift(2).map { |path| File.expand_path(path) }
args = [repo, '--no-auto-unshallow', *ARGV]
# Stabilize age fields while comparing; Git still evaluates its actual history.
clock = Time.now
Time.define_singleton_method(:now) { clock }

def measured
  start = Process.clock_gettime(Process::CLOCK_MONOTONIC)
  value = yield
  [value, Process.clock_gettime(Process::CLOCK_MONOTONIC) - start]
end

def canonical(result)
  result['meta'].delete('analyzed_at')
  result
end

output = StringIO.new
status, baseline = measured do
  StudFinder::CLI.start(args + ['--output', 'json'], stdout: output, stderr: $stderr)
end
abort 'Full scan failed' unless status.zero?
oracle = canonical(JSON.parse(output.string))
session = StudFinder::Session.new(args + ['--cache-dir', cache])
begin
  first, startup = measured { canonical(session.scan) }
  raise 'First scan differs from full analysis' unless first == oracle

  warm = Array.new(5) do
    result, elapsed = measured { canonical(session.scan) }
    raise 'Cached scan differs from full analysis' unless result == oracle

    elapsed
  end
  puts JSON.pretty_generate(full_seconds: baseline, first_session_scan_seconds: startup,
                            warm_seconds: warm, parity: true, warnings: oracle['warnings'])
ensure
  session.close
end
