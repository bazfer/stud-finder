# frozen_string_literal: true

require 'spec_helper'
require 'stud_finder/session'

RSpec.describe StudFinder::Session do
  around do |example|
    Dir.mktmpdir do |root|
      @root = root
      @cache = File.join(root, 'analysis-cache')
      system('git', 'init', '-q', root)
      system('git', '-C', root, 'config', 'user.name', 'Session Test')
      system('git', '-C', root, 'config', 'user.email', 'session@example.invalid')
      6.times { |i| File.write(File.join(root, "file_#{i}.rb"), "class File#{i}; end\n") }
      system('git', '-C', root, 'add', '.')
      system('git', '-C', root, 'commit', '-qm', 'initial')
      @args = [root, '--cache-dir', @cache, '--no-auto-unshallow']
      @session = described_class.new(@args, stderr: StringIO.new)
      example.run
    ensure
      @session&.close
    end
  end

  it 'keeps complete reports stable and changes the generation after an edit' do
    first = @session.scan
    generation = @session.generation
    second = @session.scan
    [first, second].each { |result| result['meta'].delete('analyzed_at') }
    expect(second).to eq(first)
    expect(@session.generation).to eq(generation)
    File.write(File.join(@root, 'file_0.rb'), "class File0; File1; end\n")
    edited = @session.scan
    expect(@session.generation).not_to eq(generation)
    expect(edited['ruby'].find { |row| row['path'] == 'file_1.rb' }['fan_in']).to eq(1)
  end

  it 'retries if a source changes between pipeline phases' do
    changed = false
    calls = 0
    allow(StudFinder::FanIn).to receive(:new).and_wrap_original do |constructor, **args|
      engine = constructor.call(**args)
      allow(engine).to receive(:call).and_wrap_original do |original|
        calls += 1
        result = original.call
        unless changed
          File.write(File.join(@root, 'file_0.rb'), "class File0; File1; end\n")
          changed = true
        end
        result
      end
      engine
    end
    result = @session.scan
    expect(calls).to eq(2)
    expect(result['ruby'].find { |row| row['path'] == 'file_1.rb' }['fan_in']).to eq(1)
  end

  it 'detects additions made while the pipeline is running' do
    added = false
    allow(StudFinder::FanIn).to receive(:new).and_wrap_original do |constructor, **args|
      engine = constructor.call(**args)
      allow(engine).to receive(:call).and_wrap_original do |original|
        result = original.call
        unless added
          File.write(File.join(@root, 'new_file.rb'), 'class NewFile; File1; end')
          added = true
        end
        result
      end
      engine
    end
    expect(@session.scan['ruby'].map { |row| row['path'] }).to include('new_file.rb')
  end

  it 'reads changed coverage inputs instead of reusing final scores' do
    coverage = File.join(@root, 'coverage.info')
    File.write(coverage, "SF:file_0.rb\nDA:1,1\nend_of_record\n")
    first = @session.scan(extra_args: ['--ruby-coverage', coverage])
    File.write(coverage, "SF:file_0.rb\nDA:1,0\nend_of_record\n")
    second = @session.scan(extra_args: ['--ruby-coverage', coverage])
    row = ->(report) { report['ruby'].find { |entry| entry['path'] == 'file_0.rb' } }
    expect(row.call(first)['coverage']).to eq(1.0)
    expect(row.call(second)['coverage']).to eq(0.0)
  end

  it 'uses the same coupling-aware scores in edges and normal scans' do
    2.times { |i| File.write(File.join(@root, "file_#{i}.rb"), "class File#{i}; end\n# changed\n") }
    system('git', '-C', @root, 'commit', '-qam', 'co-change')
    args = ['--coupling-min-commits', '1', '--coupling-max-commit-files', '2']
    row = @session.scan(extra_args: args)['ruby'].find { |entry| entry['path'] == 'file_0.rb' }
    output = StringIO.new
    status = StudFinder::CLI.start(['edges', 'file_0.rb', *@args, *args], stdout: output, stderr: StringIO.new)
    expect(status).to eq(0)
    expect(output.string).to include("score: #{format('%.4f', row['score'])}")
  end

  it 'rejects an edit reverted during analysis even when content and mtime match again' do
    file = File.join(@root, 'file_0.rb')
    original = File.read(file)
    stamp = File.mtime(file)
    snapshot = StudFinder::SourceSnapshot.new(root: @root, files: ['file_0.rb'], directories: {})
    File.write(file, 'class File0; File1; end')
    File.write(file, original)
    File.utime(stamp, stamp, file)
    expect { snapshot.verify! }.to raise_error(StudFinder::SourceSnapshot::Changed)
  end

  it 'rejects a missing explicit JavaScript configuration before analysis' do
    expect do
      @session.scan(extra_args: ['--js-config', File.join(@root, 'missing.json')])
    end.to raise_error(StudFinder::CLI::ValidationError)
  end

  it 'serves JSON-lines and recovers after an invalid request' do
    input = StringIO.new("invalid\n{}\n{\"args\":[\"--only\",\"file_0.rb\"]}\n")
    output = StringIO.new
    code = StudFinder::CLI.start(['serve', *@args], stdin: input, stdout: output, stderr: StringIO.new)
    messages = output.string.lines.map { |line| JSON.parse(line) }
    expect(code).to eq(0)
    expect(messages[0]).to have_key('error')
    expect(messages[1]['generation']).to match(/\A[0-9a-f]{64}\z/)
    expect(messages[1]['result']['ruby'].length).to eq(6)
    expect(messages[2]['result']['ruby'].map { |row| row['path'] }).to eq(['file_0.rb'])
  end
end
