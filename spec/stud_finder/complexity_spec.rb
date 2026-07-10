# frozen_string_literal: true

require 'json'
require 'spec_helper'
require 'stud_finder/complexity'

RSpec.describe StudFinder::Complexity do
  def status(exitstatus)
    instance_double(Process::Status, exitstatus: exitstatus)
  end

  def rubocop_json(files)
    JSON.generate('files' => files)
  end

  def complexity_offense(method_name, score)
    {
      'cop_name' => 'Metrics/CyclomaticComplexity',
      'message' => "Cyclomatic complexity for `#{method_name}` is too high. [#{score}/0]"
    }
  end

  def run_complexity(
    stdout:, status: status(0), files: ['app/models/user.rb'], repo_path: '/repo', stderr: StringIO.new
  )
    allow(Open3).to receive(:capture3).and_return([stdout, '', status])

    described_class.new(repo_path: repo_path, files: files, stderr: stderr).call
  end

  it 'parses simple-method complexity reported by the Max 0 RuboCop config' do
    stdout = rubocop_json([
                            {
                              'path' => '/repo/app/models/user.rb',
                              'offenses' => [complexity_offense('name', 1)]
                            }
                          ])

    result = run_complexity(stdout: stdout)

    expect(result.counts).to eq('app/models/user.rb' => 1)
  end

  it 'uses the maximum method complexity score per file instead of summing methods' do
    stdout = rubocop_json([
                            {
                              'path' => '/repo/app/models/user.rb',
                              'offenses' => [
                                complexity_offense('simple', 1),
                                complexity_offense('branching', 2)
                              ]
                            }
                          ])

    result = run_complexity(stdout: stdout)

    expect(result.counts).to eq('app/models/user.rb' => 2)
  end

  it 'keeps files with no methods at zero' do
    result = run_complexity(stdout: rubocop_json([]), files: ['app/models/user.rb'])

    expect(result.counts).to eq('app/models/user.rb' => 0)
  end

  it 'treats exit codes 0 and 1 as parseable' do
    [0, 1].each do |exitstatus|
      result = run_complexity(stdout: rubocop_json([]), status: status(exitstatus))

      expect(result.counts).to eq('app/models/user.rb' => 0)
    end
  end

  it 'raises on RuboCop exit code 2' do
    allow(Open3).to receive(:capture3).and_return(['{}', 'fatal parse failure', status(2)])

    expect do
      described_class.new(repo_path: '/repo', files: ['app/models/user.rb']).call
    end.to raise_error(StudFinder::Complexity::Error, /rubocop failed: fatal parse failure/)
  end

  it 'skips per-file parse errors, logs to stderr, and continues' do
    stderr = StringIO.new
    stdout = rubocop_json([
                            {
                              'path' => '/repo/app/models/bad.rb',
                              'offenses' => [{ 'cop_name' => 'Lint/Syntax', 'message' => 'unexpected token' }]
                            },
                            {
                              'path' => '/repo/app/models/good.rb',
                              'offenses' => [complexity_offense('call', 2)]
                            }
                          ])

    result = run_complexity(
      stdout: stdout,
      files: ['app/models/bad.rb', 'app/models/good.rb'],
      stderr: stderr
    )

    expect(result.counts).to eq('app/models/good.rb' => 2)
    expect(result.skipped_files).to eq(['app/models/bad.rb'])
    expect(stderr.string).to include('Warning: skipping app/models/bad.rb')
  end

  it 'uses a temporary RuboCop config that enables only cyclomatic complexity with Max 0' do
    captured_config = nil
    allow(Open3).to receive(:capture3) do |*args|
      config_path = args.fetch(args.index('--config') + 1)
      captured_config = File.read(config_path)
      [rubocop_json([]), '', status(0)]
    end

    described_class.new(repo_path: '/repo', files: ['app/models/user.rb']).call

    expect(Open3).to have_received(:capture3).with(
      'rubocop',
      '--config', kind_of(String),
      '--format', 'json',
      '--',
      'app/models/user.rb',
      chdir: '/repo'
    )
    expect(captured_config).to include('DisabledByDefault: true')
    expect(captured_config).to include('Metrics/CyclomaticComplexity:')
    expect(captured_config).to include('Enabled: true')
    expect(captured_config).to include('Max: 0')
  end

  it 'places an option terminator before filenames that start with a dash' do
    captured_args = nil
    allow(Open3).to receive(:capture3) do |*args|
      captured_args = args
      [rubocop_json([]), '', status(0)]
    end

    result = described_class.new(repo_path: '/repo', files: ['-legacy.rb']).call

    expect(result.counts).to eq('-legacy.rb' => 0)
    dash_filename_index = captured_args.index do |arg|
      arg.is_a?(String) && arg.start_with?('-') && arg.end_with?('.rb')
    end
    expect(captured_args.fetch(dash_filename_index - 1)).to eq('--')
  end

  it 'runs RuboCop against collected files in relative-path batches' do
    Dir.mktmpdir do |repo|
      files = Array.new(501) do |index|
        path = "app/models/model_#{index}.rb"
        full_path = File.join(repo, path)
        FileUtils.mkdir_p(File.dirname(full_path))
        File.write(full_path, "class Model#{index}\n  def call\n  end\nend\n")
        path
      end
      batches = []
      allow(Open3).to receive(:capture3) do |*args, **kwargs|
        batch = args.drop(args.index('--') + 1)
        batches << [batch, kwargs]
        payload = batch.map.with_index do |file, offset|
          score = file == 'app/models/model_500.rb' ? 3 : (offset % 2) + 1
          { 'path' => File.join(repo, file), 'offenses' => [complexity_offense('call', score)] }
        end
        [rubocop_json(payload), '', status(1)]
      end

      result = described_class.new(repo_path: repo, files: files).call

      expect(batches.map { |batch, _kwargs| batch.length }).to eq([500, 1])
      expect(batches).to all(satisfy do |batch, kwargs|
        kwargs == { chdir: repo } && batch.all? { |file| !file.start_with?('/') }
      end)
      expect(result.counts.length).to eq(501)
      expect(result.counts.fetch('app/models/model_0.rb')).to eq(1)
      expect(result.counts.fetch('app/models/model_499.rb')).to eq(2)
      expect(result.counts.fetch('app/models/model_500.rb')).to eq(3)
    end
  end

  it 'ignores the target repo RuboCop excludes when scoring explicit files' do
    Dir.mktmpdir do |repo|
      system('git', 'init', '-q', repo)
      FileUtils.mkdir_p(File.join(repo, 'app/models'))
      File.write(File.join(repo, '.rubocop.yml'), <<~YAML)
        AllCops:
          Exclude:
            - app/models/**
      YAML
      File.write(File.join(repo, 'app/models/user.rb'), <<~RUBY)
        class User
          def call
            if a
            elsif b
            end
          end
        end
      RUBY

      result = described_class.new(repo_path: repo, files: ['app/models/user.rb']).call

      expect(result.counts.fetch('app/models/user.rb')).to be > 0
    end
  end

  it 'raises the required message when rubocop is not in PATH' do
    allow(Open3).to receive(:capture3).and_raise(Errno::ENOENT)

    expect do
      described_class.new(repo_path: '/repo', files: ['app/models/user.rb']).call
    end.to raise_error(StudFinder::Complexity::Error, 'Error: rubocop not found. Install it: gem install rubocop')
  end
end
