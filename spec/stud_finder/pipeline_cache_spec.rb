# frozen_string_literal: true

require 'spec_helper'
require 'stud_finder/complexity'
require 'stud_finder/loc_counter'
require 'stud_finder/git_cache'
require 'stud_finder/newness'
require 'stud_finder/file_collector'

RSpec.describe 'Pipeline caches' do
  around do |example|
    Dir.mktmpdir do |root|
      @root = root
      @cache = File.join(root, 'cache')
      example.run
    end
  end

  def write(path, source)
    absolute = File.join(@root, path)
    FileUtils.mkdir_p(File.dirname(absolute))
    File.write(absolute, source)
  end

  def complexity(files, cached: true)
    StudFinder::Complexity.new(repo_path: @root, files: files, stderr: StringIO.new,
                               cache_dir: cached ? @cache : nil)
  end

  it 'matches command-line RuboCop for branches, directives, syntax errors and empty files' do
    sources = {
      'branch.rb' => 'def work(x); x ? 1 : 2; end',
      'disabled.rb' => "# rubocop:disable Metrics/CyclomaticComplexity\ndef work(x); x ? 1 : 2; end\n",
      'invalid.rb' => 'def broken(', 'empty.rb' => '',
      'block.rb' => 'define_method(:work) { |xs| xs.map { |x| x && x.foo } }'
    }
    sources.each { |file, source| write(file, source) }
    oracle = complexity(sources.keys, cached: false).call
    expect(complexity(sources.keys).call.to_h).to eq(oracle.to_h)
    expect(complexity(sources.keys).call.to_h).to eq(oracle.to_h)
  end

  it 'uses repository Ruby targets and invalidates cached complexity when the target changes' do
    write('implicit.rb', 'def work(xs); xs.map { it }; end')
    versions = %w[3.2 3.4 4.0]
    counts = versions.map do |version|
      write('.ruby-version', version)
      cached = complexity(['implicit.rb'])
      value = cached.call
      expect(value.to_h).to eq(complexity(['implicit.rb'], cached: false).call.to_h)
      expect(cached.cache_stats[:misses]).to eq(1)
      value.counts
    end
    expect(counts.first).not_to eq(counts.last)
  end

  it 'accepts anonymous block forwarding under the repository Ruby target' do
    write('.ruby-version', '4.0')
    write('forward.rb', 'def render(&); write(component do |section| section.body(&); end); end')
    oracle = complexity(['forward.rb'], cached: false).call
    expect(oracle.skipped_files).to be_empty
    expect(complexity(['forward.rb']).call.to_h).to eq(oracle.to_h)
  end

  it 'detects an effective Ruby target changing during a scan' do
    write('.ruby-version', '3.2')
    write('a.rb', 'def work(xs); xs.map { it }; end')
    snapshot = StudFinder::SourceSnapshot.new(root: @root, files: ['a.rb'], directories: {})
    write('.ruby-version', '3.4')
    expect { snapshot.verify! }.to raise_error(StudFinder::SourceSnapshot::Changed)
  end

  it 'reuses unchanged complexity and invalidates content even with unchanged length and mtime' do
    write('a.rb', 'def work(x); x ? 1 : 2; end')
    write('b.rb', 'def work; 1; end')
    complexity(%w[a.rb b.rb]).call
    warm = complexity(%w[a.rb b.rb])
    expect(warm).not_to receive(:run_embedded)
    warm.call
    stamp = File.mtime(File.join(@root, 'a.rb'))
    write('a.rb', 'def work(x); x ; 1 ; 2; end')
    File.utime(stamp, stamp, File.join(@root, 'a.rb'))
    edited = complexity(%w[a.rb b.rb])
    expect(edited.call.to_h).to eq(complexity(%w[a.rb b.rb], cached: false).call.to_h)
    expect(edited.cache_stats).to eq(hits: 1, misses: 1)
    File.rename(File.join(@root, 'a.rb'), File.join(@root, 'renamed.rb'))
    File.delete(File.join(@root, 'b.rb'))
    expect(complexity(['renamed.rb']).call.to_h).to eq(complexity(['renamed.rb'], cached: false).call.to_h)
  end

  it 'caches LOC by content across language populations' do
    write('a.rb', "a\n\nb\n")
    write('b.js', "x\ny\n")
    counter = ->(files) { StudFinder::LocCounter.new(repo_path: @root, files: files, cache_dir: @cache) }
    expect(counter.call(['a.rb']).call).to eq('a.rb' => 2)
    counter.call(['b.js']).call
    warm = counter.call(['a.rb'])
    expect(warm).not_to receive(:non_blank_lines)
    warm.call
    write('a.rb', "a\n\n\n")
    expect(counter.call(['a.rb']).call).to eq('a.rb' => 1)
  end

  it 'prunes only wholly excluded directories and preserves the selected file population' do
    system('git', 'init', '-q', @root)
    %w[src/a.rb src/b.rb src/c.rb src/d.rb src/e.rb src/keep.js].each { |path| write(path, '') }
    %w[vendor/deep/a.rb x/node_modules/deep/x.js ignored/a.rb ignored-too/keep.rb].each { |path| write(path, '') }
    opts = { path: @root, excludes: ['ignored/**', 'src/*.js'], stderr: StringIO.new }
    original = StudFinder::FileCollector.new(**opts).collect
    result = StudFinder::FileCollector.new(**opts, prune: true).collect
    expect(result.files).to eq(original.files)
    expect(result.languages).to eq(original.languages)
    expect(result.pruned_directories).to eq(4)
    expect(result.files).to include('ignored-too/keep.rb')
  end

  it 'reuses successful Git logs and invalidates them after a commit or config change' do
    system('git', 'init', '-q', @root)
    system('git', '-C', @root, 'config', 'user.name', 'Cache Test')
    system('git', '-C', @root, 'config', 'user.email', 'cache@example.invalid')
    write('a.rb', 'one')
    system('git', '-C', @root, 'add', 'a.rb')
    system('git', '-C', @root, 'commit', '-qm', 'initial')
    args = ['git', '-C', @root, 'log', '--format=%H', '--name-only']
    reader = -> { StudFinder::GitCache.new(repo_path: @root, directory: @cache) }
    first = reader.call.capture3(*args).first
    allow(Open3).to receive(:capture3).and_call_original
    expect(reader.call.capture3(*args).first).to eq(first)
    expect(Open3).not_to have_received(:capture3).with(*args)
    write('a.rb', 'two')
    system('git', '-C', @root, 'commit', '-qam', 'second')
    expect(reader.call.capture3(*args).first).not_to eq(first)
    expect(Open3).to have_received(:capture3).with(*args).once
    system('git', '-C', @root, 'config', 'core.quotepath', 'false')
    reader.call.capture3(*args)
    expect(Open3).to have_received(:capture3).with(*args).twice
  end

  it 'caches parsed history while recomputing age and invalidating changed Git history' do
    system('git', 'init', '-q', @root)
    system('git', '-C', @root, 'config', 'user.name', 'Cache Test')
    system('git', '-C', @root, 'config', 'user.email', 'cache@example.invalid')
    write('a.rb', 'one')
    system('git', '-C', @root, 'add', 'a.rb')
    system('git', '-C', @root, 'commit', '-qm', 'initial')
    now = Time.now
    reader = -> { StudFinder::GitCache.new(repo_path: @root, directory: @cache) }
    engine = lambda do |time, cache|
      StudFinder::Newness.new(repo_path: @root, files: ['a.rb'], now: time, git_cache: cache)
    end
    first = engine.call(now, reader.call).call
    cached = engine.call(now + 86_401, reader.call)
    expect(cached).not_to receive(:parse_history)
    expect(cached.call).to eq(engine.call(now + 86_401, nil).call)
    expect(cached.call['a.rb'][:age_days]).to be > first['a.rb'][:age_days]
    write('a.rb', 'two')
    system('git', '-C', @root, 'commit', '-qam', 'second')
    expect(engine.call(now, reader.call).call).to eq(engine.call(now, nil).call)
    expect(engine.call(now, reader.call).call['a.rb'][:total_commits]).to eq(2)
  end

  it 'refreshes directory inventories for additions, renames, deletions and header edits' do
    system('git', 'init', '-q', @root)
    6.times { |i| write("src/file#{i}.rb", "class File#{i}; end") }
    index = StudFinder::DirectoryIndex.new
    options = { path: @root, stderr: StringIO.new }
    collect = -> { StudFinder::FileCollector.new(**options, prune: true, directory_index: index).collect.files }
    collect.call
    allow(Dir).to receive(:children).and_call_original
    expect(collect.call).to eq(StudFinder::FileCollector.new(**options).collect.files)
    expect(Dir).not_to have_received(:children)
    write('new/nested.rb', 'class Nested; end')
    File.rename(File.join(@root, 'src/file0.rb'), File.join(@root, 'src/renamed.rb'))
    File.delete(File.join(@root, 'src/file1.rb'))
    expect(collect.call).to eq(StudFinder::FileCollector.new(**options).collect.files)
    write('src/file2.rb', '# This file is auto-generated')
    expect(collect.call).to eq(StudFinder::FileCollector.new(**options).collect.files)
    expect(collect.call).not_to include('src/file2.rb')
  end

  it 'expires a sliding-window log when a commit crosses the cutoff' do
    system('git', 'init', '-q', @root)
    system('git', '-C', @root, 'config', 'user.name', 'Cache Test')
    system('git', '-C', @root, 'config', 'user.email', 'cache@example.invalid')
    write('a.rb', '')
    system('git', '-C', @root, 'add', 'a.rb')
    system('git', '-C', @root, 'commit', '-qm', 'initial')
    now = Time.now
    args = ['git', '-C', @root, 'log', '--since=1 days ago', '--format=%H']
    StudFinder::GitCache.new(repo_path: @root, directory: @cache, now: now).capture3(*args)
    allow(Open3).to receive(:capture3).and_call_original
    StudFinder::GitCache.new(repo_path: @root, directory: @cache, now: now + 86_402).capture3(*args)
    expect(Open3).to have_received(:capture3).with(*args).once
  end
end
