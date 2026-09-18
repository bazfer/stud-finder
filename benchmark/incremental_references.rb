# frozen_string_literal: true

# Run: bundle exec ruby benchmark/incremental_references.rb [file_count]
# Measures Ruby dependency analysis only, including cache I/O. It does not
# measure CLI startup, complexity, history, scores, or editor transport.
require 'fileutils'
require 'json'
require 'stringio'
require 'tmpdir'
require_relative '../lib/stud_finder/fan_in'

count = Integer(ARGV.fetch(0, '1000'))
raise ArgumentError, 'file_count must be at least 10' if count < 10

def analyze(repo, files, cache)
  instance = StudFinder::FanIn.new(repo_path: repo, files: files, cache_dir: cache, stderr: StringIO.new)
  started = Process.clock_gettime(Process::CLOCK_MONOTONIC)
  result = instance.call
  seconds = Process.clock_gettime(Process::CLOCK_MONOTONIC) - started
  [result, { seconds: seconds, **instance.cache_stats }]
end

def distribution(samples)
  times = samples.map { |sample| sample[:seconds] }.sort
  { samples: samples.length, p50_seconds: times[(times.length * 0.5).ceil - 1],
    p95_seconds: times[(times.length * 0.95).ceil - 1], last_cache_stats: samples.last }
end

Dir.mktmpdir('stud-finder-benchmark') do |root|
  repo = File.join(root, 'repo')
  cache = File.join(root, 'cache')
  FileUtils.mkdir_p(File.join(repo, 'app/models'))
  files = Array.new(count) do |index|
    path = "app/models/model_#{index}.rb"
    refs = 1.upto(5).map { |offset| "Model#{(index + offset) % count}.new" }.join("\n    ")
    File.write(File.join(repo, path), "class Model#{index}\n  def call\n    #{refs}\n  end\nend\n")
    path
  end
  expected, clean = analyze(repo, files, nil)
  actual, cold = analyze(repo, files, cache)
  raise 'cold parity mismatch' unless expected.to_h == actual.to_h

  warm = Array.new(10) do
    actual, sample = analyze(repo, files, cache)
    raise 'warm parity mismatch' unless expected.to_h == actual.to_h

    sample
  end
  edits = Array.new(10) do |index|
    File.write(File.join(repo, files[index]), "class Model#{index}; Model#{count - 1}.new; end\n")
    actual, sample = analyze(repo, files, cache)
    oracle, = analyze(repo, files, nil)
    raise 'edit parity mismatch' unless oracle.to_h == actual.to_h
    raise 'unexpected edit invalidation' unless sample[:misses] == 1

    sample
  end
  puts JSON.pretty_generate(ruby: RUBY_DESCRIPTION, files: count, scope: 'Ruby reference analysis only',
                            clean: clean, cold_cache: cold, warm: distribution(warm), edits: distribution(edits),
                            parity: 'all replay steps matched fresh analysis')
end
