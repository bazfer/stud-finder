# frozen_string_literal: true

require 'csv'
require 'spec_helper'
require 'stud_finder/cli'

RSpec.describe StudFinder::CLI do
  full_weights = 'fan_in:0.25,fan_out:0.10,complexity:0.25,churn:0.25,coverage:0.15'

  define_method(:full_weights) { full_weights }

  def run_cli(argv)
    stdout = StringIO.new
    stderr = StringIO.new
    status = described_class.new(argv, stdout: stdout, stderr: stderr).run
    [status, stdout.string, stderr.string]
  end

  def make_repo(file_count: 5)
    Dir.mktmpdir do |dir|
      system('git', 'init', '-q', dir)
      system('git', '-C', dir, 'config', 'user.email', 'stud-finder@example.test')
      system('git', '-C', dir, 'config', 'user.name', 'Stud Finder')
      file_count.times do |i|
        path = File.join(dir, "app/models/model_#{i}.rb")
        FileUtils.mkdir_p(File.dirname(path))
        File.write(path, "class Model#{i}\nend\n")
      end
      system('git', '-C', dir, 'add', '.')
      system('git', '-C', dir, 'commit', '-qm', 'initial')
      yield dir
    end
  end

  def write_coverage_report(dir, files)
    path = File.join(dir, 'coverage.xml')
    classes = files.map do |file|
      %(<class filename="#{file}" line-rate="0.5" />)
    end.join("\n")
    File.write(path, <<~XML)
      <coverage><packages><package><classes>
      #{classes}
      </classes></package></packages></coverage>
    XML
    path
  end

  def write_lcov_report(dir, files)
    path = File.join(dir, 'lcov.info')
    File.write(path, files.map do |file|
      <<~LCOV
        SF:#{file}
        LF:2
        LH:1
        end_of_record
      LCOV
    end.join)
    path
  end

  def write_resultset_report(dir, files)
    path = File.join(dir, 'resultset.json')
    coverage = files.to_h { |file| [file, { 'lines' => [1, 0] }] }
    File.write(path, JSON.generate('RSpec' => { 'coverage' => coverage }))
    path
  end

  it 'prints help' do
    stdout = StringIO.new
    cli = described_class.new(['--help'], stdout: stdout, stderr: StringIO.new)

    expect { cli.run }.to raise_error(SystemExit) do |error|
      expect(error.status).to eq(0)
    end
    expect(stdout.string).to include('Usage: stud-finder [PATH] [OPTIONS]')
    expect(stdout.string).to include('--weights')
    expect(stdout.string).to include('--churn-days N')
    expect(stdout.string).to include('default: 180')
    expect(stdout.string).to include('--ruby-coverage')
    expect(stdout.string).to include('--js-coverage')
    expect(stdout.string).to include('--js-timeout')
    expect(stdout.string).to include('--coverage')
  end

  it 'prints version' do
    stdout = StringIO.new
    cli = described_class.new(['--version'], stdout: stdout, stderr: StringIO.new)

    expect { cli.run }.to raise_error(SystemExit) do |error|
      expect(error.status).to eq(0)
    end
    expect(stdout.string).to include(StudFinder::VERSION)
  end

  it 'emits a deprecation warning when --coverage alias is used' do
    make_repo(file_count: 5) do |root|
      files = Array.new(5) { |i| "app/models/model_#{i}.rb" }
      coverage = write_coverage_report(root, files)
      allow_any_instance_of(StudFinder::Complexity).to receive(:call).and_return(
        StudFinder::Complexity::Result.new(counts: files.to_h { |file| [file, 0] }, skipped_files: [])
      )
      allow_any_instance_of(StudFinder::Churn).to receive(:call).and_return(
        StudFinder::Churn::Result.new(counts: files.to_h { |file| [file, 0] }, zero_inflated: false, zero_percentage: 0)
      )

      status, _stdout, stderr = run_cli([root, '--min-files', '5', '--coverage', coverage, '--output', 'json'])

      expect(status).to eq(0)
      expect(stderr).to include('coverage_flag_deprecated')
    end
  end

  it 'validates --js-coverage and --js-timeout' do
    missing = File.join(Dir.tmpdir, "stud-finder-js-coverage-nope-#{rand(100_000)}.info")
    status, _stdout, stderr = run_cli(['--js-coverage', missing])
    expect(status).to eq(1)
    expect(stderr).to include("Error: JS coverage file not found: #{missing}")

    status, _stdout, stderr = run_cli(['--js-timeout', '0'])
    expect(status).to eq(1)
    expect(stderr).to include('--js-timeout must be positive')
  end

  it 'validates weights that do not sum to 1.0' do
    status, _stdout, stderr = run_cli(['--weights', 'fan_in:0.4,fan_out:0.0,complexity:0.3,churn:0.1,coverage:0.0'])

    expect(status).to eq(1)
    expect(stderr).to include('actual sum is 0.8000')
  end

  it 'rejects coverage weight in Phase 1' do
    status, _stdout, stderr = run_cli(['--weights', full_weights])

    expect(status).to eq(1)
    expect(stderr).to include('coverage weight must be 0.0 when no coverage data is provided')
  end

  it 'requires all weight keys including fan_out' do
    status, _stdout, stderr = run_cli(['--weights', 'fan_in:0.5,complexity:0.3,churn:0.2'])

    expect(status).to eq(1)
    expect(stderr).to include('weights must include fan_in, fan_out, complexity, churn, and coverage')
  end

  it 'reports a missing coverage file' do
    path = File.join(Dir.tmpdir, "stud-finder-coverage-nope-#{rand(100_000)}.xml")
    status, _stdout, stderr = run_cli(['--coverage', path])

    expect(status).to eq(1)
    expect(stderr).to include("Error: coverage file not found: #{path}")
  end

  it 'accepts a non-zero coverage weight when coverage is provided' do
    make_repo(file_count: 5) do |root|
      files = Array.new(5) { |i| "app/models/model_#{i}.rb" }
      coverage = write_coverage_report(root, files)
      allow_any_instance_of(StudFinder::Complexity).to receive(:call).and_return(
        StudFinder::Complexity::Result.new(counts: files.to_h { |file| [file, 0] }, skipped_files: [])
      )
      allow_any_instance_of(StudFinder::Churn).to receive(:call).and_return(
        StudFinder::Churn::Result.new(counts: files.to_h { |file| [file, 0] }, zero_inflated: false, zero_percentage: 0)
      )

      status, stdout, stderr = run_cli([
                                         root,
                                         '--min-files', '5',
                                         '--weights', full_weights,
                                         '--coverage', coverage,
                                         '--output', 'json'
                                       ])

      expect(status).to eq(0)
      expect(stderr).not_to include('coverage weight must be 0.0')
      expect(JSON.parse(stdout).fetch('ruby').map { |file| file['coverage'] }.uniq).to eq([0.5])
    end
  end

  it 'detects and uses LCOV reports from --coverage' do
    make_repo(file_count: 5) do |root|
      files = Array.new(5) { |i| "app/models/model_#{i}.rb" }
      coverage = write_lcov_report(root, files)
      allow_any_instance_of(StudFinder::Complexity).to receive(:call).and_return(
        StudFinder::Complexity::Result.new(counts: files.to_h { |file| [file, 0] }, skipped_files: [])
      )
      allow_any_instance_of(StudFinder::Churn).to receive(:call).and_return(
        StudFinder::Churn::Result.new(counts: files.to_h { |file| [file, 0] }, zero_inflated: false, zero_percentage: 0)
      )

      status, stdout, stderr = run_cli([root, '--min-files', '5', '--coverage', coverage, '--output', 'json'])

      expect(status).to eq(0), stderr
      expect(JSON.parse(stdout).fetch('ruby').map { |file| file['coverage'] }.uniq).to eq([0.5])
    end
  end

  it 'detects and uses SimpleCov resultset reports from --coverage' do
    make_repo(file_count: 5) do |root|
      files = Array.new(5) { |i| "app/models/model_#{i}.rb" }
      coverage = write_resultset_report(root, files)
      allow_any_instance_of(StudFinder::Complexity).to receive(:call).and_return(
        StudFinder::Complexity::Result.new(counts: files.to_h { |file| [file, 0] }, skipped_files: [])
      )
      allow_any_instance_of(StudFinder::Churn).to receive(:call).and_return(
        StudFinder::Churn::Result.new(counts: files.to_h { |file| [file, 0] }, zero_inflated: false, zero_percentage: 0)
      )

      status, stdout, stderr = run_cli([root, '--min-files', '5', '--coverage', coverage, '--output', 'json'])

      expect(status).to eq(0), stderr
      expect(JSON.parse(stdout).fetch('ruby').map { |file| file['coverage'] }.uniq).to eq([0.5])
    end
  end

  it 'validates threshold ranges' do
    status, _stdout, stderr = run_cli(['--trunk-threshold', '100'])

    expect(status).to eq(1)
    expect(stderr).to include('trunk-threshold must be between 1 and 99')
  end

  it 'requires branch threshold to be less than trunk threshold' do
    status, _stdout, stderr = run_cli(['--trunk-threshold', '50', '--branch-threshold', '50'])

    expect(status).to eq(1)
    expect(stderr).to include('branch-threshold must be strictly less than trunk-threshold')
  end

  it 'returns path errors as exit 1 messages' do
    status, _stdout, stderr = run_cli([File.join(Dir.tmpdir, "stud-finder-nope-#{rand(100_000)}")])

    expect(status).to eq(1)
    expect(stderr).to include('does not exist')
  end

  it 'collects files and emits scored table output' do
    make_repo(file_count: 5) do |root|
      files = Array.new(5) { |i| "app/models/model_#{i}.rb" }
      allow_any_instance_of(StudFinder::Complexity).to receive(:call).and_return(
        StudFinder::Complexity::Result.new(counts: files.to_h { |file| [file, 0] }, skipped_files: [])
      )
      allow_any_instance_of(StudFinder::Churn).to receive(:call).and_return(
        StudFinder::Churn::Result.new(counts: files.to_h { |file| [file, 0] }, zero_inflated: false, zero_percentage: 0)
      )

      status, stdout, stderr = run_cli([root, '--min-files', '5'])

      expect(status).to eq(0)
      expect(stderr).to include('Score uses 4-factor formula')
      expect(stderr).to include('computing temporal coupling (git log, 180 days)...')
      expect(stdout).to include('JavaScript/TypeScript')
      expect(stdout).to include('5 files analyzed')
      expect(stdout).to match(/rank\s+language\s+file\s+score/)
      expect(stdout).to include('ruby')
      expect(stdout).to include('score')
      expect(stdout).to include('complexity')
      expect(stdout).to include('churn_commits')
      expect(stdout).to include('churn_lines')
      expect(stdout).to include('churn_pct')
    end
  end

  it 'emits progress to stderr without changing stdout output' do
    make_repo(file_count: 5) do |root|
      files = Array.new(5) { |i| "app/models/model_#{i}.rb" }
      analyzed_files = files.drop(1)
      allow_any_instance_of(StudFinder::Complexity).to receive(:call).and_return(
        StudFinder::Complexity::Result.new(
          counts: analyzed_files.to_h { |file| [file, 0] },
          skipped_files: [files.first]
        )
      )
      allow_any_instance_of(StudFinder::Churn).to receive(:call).and_return(
        StudFinder::Churn::Result.new(
          counts: analyzed_files.to_h { |file| [file, 0] },
          zero_inflated: false,
          zero_percentage: 0
        )
      )

      status, stdout, stderr = run_cli([root, '--min-files', '5', '--churn-days', '12', '--output', 'json'])

      expect(status).to eq(0)
      expect(JSON.parse(stdout).fetch('ruby').length).to eq(4)
      expect(stderr).to include("stud-finder → collecting files... 5 found\n")
      expect(stderr).to include("stud-finder → computing Ruby fan_in + fan_out (rubocop-ast)...\n")
      expect(stderr).to include("stud-finder → computing Ruby complexity (rubocop)...\n")
      expect(stderr).to include("stud-finder → computing Ruby churn (git log, 12 days)...\n")
      expect(stderr).to include("stud-finder → normalizing + scoring 4 files...\n")
      expect(stderr).to include("stud-finder → done\n")
    end
  end

  it 'emits spreadsheet-ready csv output' do
    make_repo(file_count: 5) do |root|
      comma_path = File.join(root, 'app/models/model,with_comma.rb')
      FileUtils.rm_f(File.join(root, 'app/models/model_0.rb'))
      File.write(comma_path, "class ModelWithComma\nend\n")
      file = 'app/models/model,with_comma.rb'
      files = Array.new(4) { |i| "app/models/model_#{i + 1}.rb" } + [file]

      complexity_counts = files.to_h { |path| [path, path == file ? 7 : 0] }
      allow_any_instance_of(StudFinder::Complexity).to receive(:call).and_return(
        StudFinder::Complexity::Result.new(counts: complexity_counts, skipped_files: [])
      )
      allow_any_instance_of(StudFinder::Churn).to receive(:call).and_return(
        StudFinder::Churn::Result.new(
          counts: files.to_h { |path| [path, path == file ? 3 : 0] },
          line_counts: files.to_h { |path| [path, path == file ? 15 : 0] },
          zero_inflated: false,
          zero_percentage: 0
        )
      )

      status, stdout, stderr = run_cli([root, '--min-files', '5', '--top', '1', '--output', 'csv'])
      lines = stdout.lines
      rows = CSV.parse(stdout, nil_value: '')

      expect(status).to eq(0)
      expect(stderr).to include('Score uses 4-factor formula')
      expect(lines.length).to eq(2)
      expect(lines.first).to eq(
        "#{StudFinder::CLI::RESULT_COLUMNS.join(',')}\n"
      )
      expect(lines.last).to include('"app/models/model,with_comma.rb"')
      expect(rows.first).to eq(
        StudFinder::CLI::RESULT_COLUMNS
      )
      expect(rows.last).to eq(
        ['1', 'ruby', file, '0.5882', 'leaf', '0', '0.0000', '0', '0.0000', '0.0000', '0.0000', '7', '1.0000',
         '3', '15', '1.0000', '0.0000', '0', '0.0000', '']
      )
      expect(lines.last).to end_with(",\"\"\n")
    end
  end

  it 'computes temporal coupling in the main scan and emits nonzero coupling columns' do
    make_repo(file_count: 5) do |root|
      # Co-change model_0 and model_1 together repeatedly so they cross the threshold.
      3.times do |i|
        File.open(File.join(root, 'app/models/model_0.rb'), 'a') { |file| file.puts "# m0 change #{i}" }
        File.open(File.join(root, 'app/models/model_1.rb'), 'a') { |file| file.puts "# m1 change #{i}" }
        system('git', '-C', root, 'add', '.')
        system('git', '-C', root, 'commit', '-qm', "couple #{i}")
      end

      status, stdout, stderr = run_cli([
                                         root, '--min-files', '5', '--output', 'csv',
                                         '--coupling-min-commits', '2', '--coupling-threshold', '0.0'
                                       ])

      expect(status).to eq(0), stderr
      expect(stderr).to include('computing temporal coupling (git log, 180 days)...')
      rows = CSV.parse(stdout, nil_value: '', headers: true)
      coupled = rows.select { |row| %w[app/models/model_0.rb app/models/model_1.rb].include?(row['file']) }
      expect(coupled.length).to eq(2)
      coupled.each do |row|
        expect(row['max_coupling'].to_f).to be > 0.0
        expect(row['coupling_partners'].to_i).to be >= 1
      end
    end
  end

  it 'surfaces too few file threshold failures' do
    make_repo(file_count: 4) do |root|
      status, _stdout, stderr = run_cli([root])

      expect(status).to eq(1)
      expect(stderr).to include('Too few for meaningful analysis')
    end
  end
  it 'rejects --coverage with a non-existent file' do
    missing = File.join(Dir.tmpdir, "stud-finder-coverage-nope-#{rand(100_000)}.xml")
    status, _stdout, stderr = run_cli(['--coverage', missing])

    expect(status).to eq(1)
    expect(stderr).to include("Error: coverage file not found: #{missing}")
  end

  it 'rejects --diff-base combined with --only' do
    status, _stdout, stderr = run_cli(['--diff-base', 'origin/staging', '--only', 'app/models/user.rb'])

    expect(status).to eq(1)
    expect(stderr).to include('--diff-base and --only are mutually exclusive')
  end

  it 'exits with a clear error when the --diff-base ref is unknown' do
    make_repo(file_count: 5) do |root|
      status, _stdout, stderr = run_cli([root, '--min-files', '5', '--diff-base', 'origin/missing-branch'])

      expect(status).to eq(1)
      expect(stderr).to include('diff base ref not found')
    end
  end

  it 'errors on unknown --diff-base before running the full analysis' do
    make_repo(file_count: 5) do |root|
      status, _stdout, stderr = run_cli([root, '--min-files', '5', '--diff-base', 'origin/missing-branch'])

      expect(status).to eq(1)
      expect(stderr).to include('diff base ref not found')
      expect(stderr).not_to include('computing Ruby')
    end
  end

  it 'emits diff_filter_empty warning when the diff is empty' do
    make_repo(file_count: 5) do |root|
      system('git', '-C', root, 'branch', 'base')
      status, stdout, stderr = run_cli([root, '--min-files', '5', '--output', 'json', '--diff-base', 'base'])
      payload = JSON.parse(stdout)

      expect(status).to eq(0)
      expect(stderr).to include('diff contains no changed files')
      expect(payload['warnings']).to include('diff_filter_empty')
    end
  end

  # Regression: row paths are relative to the analysis root, but diff paths are
  # repo-root-relative. When PATH is a subdirectory they must be rebased to match,
  # else every changed file is silently dropped.
  it 'matches changed files when analyzing a subdirectory with --diff-base' do
    make_repo(file_count: 5) do |root|
      system('git', '-C', root, 'branch', 'base')
      File.write(File.join(root, 'app/models/model_0.rb'), "class Model0\n  def x = 1\nend\n")
      system('git', '-C', root, 'commit', '-qam', 'change model_0')

      status, stdout, stderr = run_cli([File.join(root, 'app'), '--min-files', '5',
                                        '--diff-base', 'base', '--output', 'json'])

      expect(status).to eq(0), stderr
      paths = JSON.parse(stdout)['ruby'].map { |row| row['path'] }
      expect(paths).to eq(['models/model_0.rb'])
      expect(stderr).not_to include('no scored files matched')
    end
  end

  it 'accepts positive coverage weight when --coverage is provided' do
    make_repo(file_count: 5) do |root|
      coverage_path = File.join(root, 'coverage.xml')
      files = Array.new(5) { |i| "app/models/model_#{i}.rb" }
      File.write(coverage_path, <<~XML)
        <coverage><packages><package><classes>
          #{files.map { |file| %(<class filename="#{file}" line-rate="0.5" />) }.join}
        </classes></package></packages></coverage>
      XML
      allow_any_instance_of(StudFinder::Complexity).to receive(:call).and_return(
        StudFinder::Complexity::Result.new(counts: files.to_h { |file| [file, 0] }, skipped_files: [])
      )
      allow_any_instance_of(StudFinder::Churn).to receive(:call).and_return(
        StudFinder::Churn::Result.new(counts: files.to_h { |file| [file, 0] }, zero_inflated: false, zero_percentage: 0)
      )

      status, _stdout, stderr = run_cli([
                                          root, '--min-files', '5', '--coverage', coverage_path,
                                          '--weights', full_weights
                                        ])

      expect(status).to eq(0), stderr
      expect(stderr).not_to include('coverage weight must be 0.0')
    end
  end

  describe 'edges subcommand' do
    it 'emits the edges report for a scored file' do
      make_repo(file_count: 5) do |root|
        status, stdout, stderr = run_cli(['edges', 'app/models/model_0.rb', root, '--min-files', '5'])

        expect(status).to eq(0), stderr
        expect(stdout).to include('stud-finder edges — app/models/model_0.rb')
        expect(stdout).to include('Temporal Coupling')
      end
    end

    # Regression: the edges subcommand used to return before option parsing ran,
    # so these flags were silently ignored (and the first flag was eaten as PATH).
    it 'honors --coupling-min-commits, --coupling-threshold, and --churn-days' do
      make_repo(file_count: 5) do |root|
        status, stdout, stderr = run_cli([
                                           'edges', 'app/models/model_0.rb', root, '--min-files', '5',
                                           '--coupling-min-commits', '1', '--coupling-threshold', '0.50',
                                           '--churn-days', '30'
                                         ])

        expect(status).to eq(0), stderr
        expect(stdout).to include('30-day window, min 1 co-changes, threshold 0.50')
      end
    end

    it 'does not consume a flag as the PATH argument when flags precede it' do
      make_repo(file_count: 5) do |root|
        status, stdout, stderr = run_cli(['edges', 'app/models/model_0.rb', '--min-files', '5', root])

        expect(status).to eq(0), stderr
        expect(stdout).to include('stud-finder edges — app/models/model_0.rb')
        expect(stderr).not_to include('does not exist')
      end
    end

    it 'keeps temporal coupling partners when analyzing a subdirectory' do
      make_repo(file_count: 5) do |root|
        2.times do |i|
          File.open(File.join(root, 'app/models/model_0.rb'), 'a') { |file| file.puts "# model 0 change #{i}" }
          File.open(File.join(root, 'app/models/model_1.rb'), 'a') { |file| file.puts "# model 1 change #{i}" }
          system('git', '-C', root, 'add', '.')
          system('git', '-C', root, 'commit', '-qm', "couple models #{i}")
        end

        status, stdout, stderr = run_cli([
                                           'edges', 'models/model_0.rb', File.join(root, 'app'),
                                           '--min-files', '5', '--coupling-min-commits', '2',
                                           '--coupling-threshold', '0.0'
                                         ])

        expect(status).to eq(0), stderr
        expect(stdout).to include('stud-finder edges — models/model_0.rb')
        expect(stdout).to include('models/model_1.rb')
        expect(stdout).not_to include('(none above threshold)')
      end
    end

    it 'rejects a non-positive --coupling-min-commits' do
      make_repo(file_count: 5) do |root|
        status, _stdout, stderr = run_cli(['edges', 'app/models/model_0.rb', root, '--coupling-min-commits', '0'])

        expect(status).to eq(1)
        expect(stderr).to include('--coupling-min-commits must be positive')
      end
    end

    it 'rejects an out-of-range --coupling-threshold' do
      make_repo(file_count: 5) do |root|
        status, _stdout, stderr = run_cli(['edges', 'app/models/model_0.rb', root, '--coupling-threshold', '1.5'])

        expect(status).to eq(1)
        expect(stderr).to include('--coupling-threshold must be between 0.0 and 1.0')
      end
    end
  end
end
