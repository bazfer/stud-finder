# frozen_string_literal: true

require 'json'
require 'open3'
require 'spec_helper'
require 'stud_finder/cli'

RSpec.describe 'fixture repo integration' do
  let(:source_fixture) { File.expand_path('../fixtures/sample_app', __dir__) }
  let(:repo_path) { Dir.mktmpdir('stud-finder-sample-app') }
  let(:bin_path) { File.expand_path('../../bin/stud-finder', __dir__) }

  before do
    FileUtils.cp_r(Dir.glob(File.join(source_fixture, '*')), repo_path)
    initialize_git_repo
  end

  after do
    FileUtils.remove_entry(repo_path)
  end

  it 'ranks the sample app and emits valid table output' do
    stdout, stderr, status = run_cli('--min-files', '5')

    expect(status).to be_success, stderr
    expect(stdout).to include('rank')
    expect(stdout).to include('score')
    expect(stdout).to include('class')
    expect(stdout).to include('JavaScript/TypeScript')
    expect(top_table_row(stdout)).to include('app/models/user.rb')
    expect(top_table_score(stdout)).to be > 0.0
  end

  it 'emits normative JSON output with ranked scores' do
    stdout, stderr, status = run_cli('--min-files', '5', '--output', 'json')
    payload = JSON.parse(stdout)

    expect(status).to be_success, stderr
    expect(payload.keys).to contain_exactly('meta', 'warnings', 'ruby', 'javascript')
    expect(payload['meta'].keys).to include('repo', 'analyzed_at', 'churn_days', 'file_count', 'files_skipped',
                                            'formula', 'weights', 'warnings')
    expect(payload['warnings']).to include('coverage_unavailable')
    expect(payload['meta']['warnings']).to eq(payload['warnings'])
    expect(payload['javascript']).to eq([])

    files = payload.fetch('ruby')
    expect(files.first['language']).to eq('ruby')
    expect(files.first['path']).to eq('app/models/user.rb')
    expect(files.first['score']).to be > 0.0
    expect(files.map { |file| file['score'] }).to all(be_between(0.0, 1.0).inclusive)
    expect(files.map { |file| file['score'] }).to eq(files.map { |file| file['score'] }.sort.reverse)
    expect(files.first.keys).to include('rank', 'language', 'path', 'score', 'class', 'fan_in', 'fan_in_pct',
                                        'complexity', 'complexity_pct', 'churn_commits', 'churn_lines', 'churn_pct',
                                        'coverage')
  end

  it 'emits JSON output with Cobertura coverage integrated' do
    coverage_path = File.join(repo_path, 'coverage/coverage.xml')
    stdout, stderr, status = run_cli('--min-files', '5', '--output', 'json', '--ruby-coverage', coverage_path)
    payload = JSON.parse(stdout)

    expect(status).to be_success, stderr
    files = payload['ruby'].to_h { |file| [file['path'], file] }
    expect(files['app/models/user.rb']['coverage']).to eq(1.0)
    expect(files['app/services/auth_service.rb']['coverage']).to eq(0.0)
    expect(files['app/services/auth_service.rb']['score']).to be > files['app/services/post_service.rb']['score']
  end

  it 'scores files absent from a partial Cobertura report with the four-factor formula' do
    coverage_path = File.join(repo_path, 'coverage/coverage.xml')
    coverage_xml = File.read(coverage_path)
    File.write(coverage_path, coverage_xml.sub(%r{\s*<class filename="app/models/post\.rb" line-rate="0\.95" />}, ''))

    stdout, stderr, status = run_cli('--min-files', '5', '--output', 'json', '--ruby-coverage', coverage_path)
    payload = JSON.parse(stdout)

    expect(status).to be_success, stderr
    files = payload['ruby'].to_h { |file| [file['path'], file] }
    expect(files['app/models/post.rb']['coverage']).to eq(0.0)
    expect(files['app/models/post.rb']['score']).to be_within(0.0001).of(0.4611)
    expect(files['app/models/profile.rb']['coverage']).to eq(0.75)
    expect(files['app/models/profile.rb']['score']).to be_within(0.0001).of(0.3097)
  end

  it 'emits markdown output' do
    stdout, stderr, status = run_cli('--min-files', '5', '--output', 'markdown')

    expect(status).to be_success, stderr
    expect(stdout).to include("| #{StudFinder::CLI::MARKDOWN_COLUMNS.join(' | ')} |")
    expect(stdout).to include('| 1 | ruby | app/models/user.rb |')
  end

  it 'filters output to files changed vs --diff-base, preserving full-repo rank and score' do
    system('git', '-C', repo_path, 'branch', '-D', 'base', out: File::NULL, err: File::NULL)
    system('git', '-C', repo_path, 'branch', 'base')
    File.open(File.join(repo_path, 'app/models/post.rb'), 'a') { |file| file.puts "\n# edit" }
    system('git', '-C', repo_path, 'add', 'app/models/post.rb')
    system('git', '-C', repo_path, 'commit', '-qm', 'edit post')

    full = JSON.parse(run_cli('--min-files', '5', '--output', 'json').first)
    stdout, stderr, status = run_cli('--min-files', '5', '--output', 'json', '--diff-base', 'base')
    filtered = JSON.parse(stdout)

    expect(status).to be_success, stderr
    emitted = (filtered['ruby'] + filtered['javascript']).map { |file| file['path'] }
    expect(emitted).to contain_exactly('app/models/post.rb')

    full_post = full['ruby'].find { |file| file['path'] == 'app/models/post.rb' }
    filtered_post = filtered['ruby'].first
    expect(filtered_post['rank']).to eq(full_post['rank'])
    expect(filtered_post['score']).to eq(full_post['score'])
    expect(filtered['meta']['filtered']).to be(true)
    expect(filtered['meta']['diff_base']).to eq('base')
  end

  it 'filters output to explicit --only paths and records them in meta' do
    stdout, stderr, status = run_cli('--min-files', '5', '--output', 'json',
                                     '--only', 'app/models/user.rb,app/models/post.rb')
    payload = JSON.parse(stdout)

    expect(status).to be_success, stderr
    expect(payload['ruby'].map { |file| file['path'] }).to contain_exactly('app/models/user.rb',
                                                                           'app/models/post.rb')
    expect(payload['meta']['filtered']).to be(true)
    expect(payload['meta']).not_to have_key('diff_base')
    expect(payload['meta']['only_paths']).to contain_exactly('app/models/user.rb', 'app/models/post.rb')
  end

  it 'applies --top to the filtered set, not the full set' do
    system('git', '-C', repo_path, 'branch', '-D', 'base', out: File::NULL, err: File::NULL)
    system('git', '-C', repo_path, 'branch', 'base')
    %w[app/models/post.rb app/models/user.rb app/models/profile.rb].each do |f|
      File.open(File.join(repo_path, f), 'a') { |file| file.puts "\n# edit" }
    end
    system('git', '-C', repo_path, 'add', '.')
    system('git', '-C', repo_path, 'commit', '-qm', 'edit multiple')

    stdout, stderr, status = run_cli('--min-files', '5', '--output', 'json',
                                     '--diff-base', 'base', '--top', '2')
    payload = JSON.parse(stdout)

    expect(status).to be_success, stderr
    expect(payload['ruby'].length).to eq(2)
    expect(payload['ruby'].map { |f| f['rank'] }).to eq(payload['ruby'].map { |f| f['rank'] }.sort)
  end

  it 'emits filter note in markdown output' do
    system('git', '-C', repo_path, 'branch', '-D', 'base', out: File::NULL, err: File::NULL)
    system('git', '-C', repo_path, 'branch', 'base')
    File.open(File.join(repo_path, 'app/models/post.rb'), 'a') { |file| file.puts "\n# edit" }
    system('git', '-C', repo_path, 'add', '.')
    system('git', '-C', repo_path, 'commit', '-qm', 'edit post')

    stdout, stderr, status = run_cli('--min-files', '5', '--output', 'markdown', '--diff-base', 'base')

    expect(status).to be_success, stderr
    expect(stdout).to include('Filtered to files changed vs base')
    expect(stdout).to include('ranks are against the full repo')
  end

  it 'warns and sets diff_no_scored_files when diff touches only unscorable files' do
    system('git', '-C', repo_path, 'branch', '-D', 'base', out: File::NULL, err: File::NULL)
    system('git', '-C', repo_path, 'branch', 'base')
    File.write(File.join(repo_path, 'README.md'), '# updated')
    system('git', '-C', repo_path, 'add', 'README.md')
    system('git', '-C', repo_path, 'commit', '-qm', 'update readme')

    stdout, stderr, status = run_cli('--min-files', '5', '--output', 'json', '--diff-base', 'base')
    payload = JSON.parse(stdout)

    expect(status).to be_success, stderr
    expect(stderr).to include('no scored files matched the diff')
    expect(payload['warnings']).to include('diff_no_scored_files')
    expect(payload['ruby']).to eq([])
    expect(payload['javascript']).to eq([])
  end

  def run_cli(*args)
    Open3.capture3('bundle', 'exec', 'ruby', bin_path, repo_path, *args)
  end

  def initialize_git_repo
    system('git', 'init', '-q', repo_path)
    system('git', '-C', repo_path, 'config', 'user.email', 'stud-finder@example.test')
    system('git', '-C', repo_path, 'config', 'user.name', 'Stud Finder')
    system('git', '-C', repo_path, 'add', '.')
    system('git', '-C', repo_path, 'commit', '-qm', 'initial sample app')
    File.open(File.join(repo_path, 'app/models/user.rb'), 'a') { |file| file.puts "\n# churn marker" }
    system('git', '-C', repo_path, 'add', 'app/models/user.rb')
    system('git', '-C', repo_path, 'commit', '-qm', 'touch user model')
    File.open(File.join(repo_path, 'app/models/user.rb'), 'a') { |file| file.puts '# another churn marker' }
    system('git', '-C', repo_path, 'add', 'app/models/user.rb')
    system('git', '-C', repo_path, 'commit', '-qm', 'touch user model again')
  end

  def top_table_row(stdout)
    stdout.lines.find { |line| line.match?(/^\s*1\s+/) }
  end

  def top_table_score(stdout)
    top_table_row(stdout).split[3].to_f
  end
end
