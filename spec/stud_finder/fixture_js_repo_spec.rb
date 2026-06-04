# frozen_string_literal: true

require 'json'
require 'open3'
require 'spec_helper'

RSpec.describe 'JavaScript fixture repo integration' do
  before(:all) do
    @repo_path = Dir.mktmpdir('stud-finder-sample-js-app')
    fixture = File.expand_path('../fixtures/sample_js_app', __dir__)
    FileUtils.cp_r(Dir.glob(File.join(fixture, '*'), File::FNM_DOTMATCH).reject do |path|
      ['.', '..'].include?(File.basename(path))
    end, @repo_path)
    system('git', 'init', '-q', @repo_path)
    system('git', '-C', @repo_path, 'config', 'user.email', 'stud-finder@example.test')
    system('git', '-C', @repo_path, 'config', 'user.name', 'Stud Finder')
    system('git', '-C', @repo_path, 'add', '.')
    system('git', '-C', @repo_path, 'commit', '-qm', 'initial js sample app')
  end

  after(:all) do
    FileUtils.remove_entry(@repo_path) if @repo_path && File.exist?(@repo_path)
  end

  it 'ranks the highest fan-in JavaScript file first and emits split JSON' do
    bin_path = File.expand_path('../../bin/stud-finder', __dir__)
    stdout, stderr, status = Open3.capture3('bundle', 'exec', 'ruby', bin_path, @repo_path, '--min-files', '5',
                                            '--output', 'json')
    payload = JSON.parse(stdout)

    expect(status).to be_success, stderr
    expect(payload.keys).to contain_exactly('meta', 'warnings', 'ruby', 'javascript')
    expect(payload['warnings']).to include('coverage_unavailable')
    expect(payload.fetch('ruby')).to eq([])
    expect(payload.fetch('javascript').first['language']).to eq('javascript')
    expect(payload.fetch('javascript').first['path']).to eq('src/hub.js')
    expect(payload.fetch('javascript').first['fan_in']).to eq(3)
    expect(payload.fetch('javascript').first['fan_out']).to be_a(Integer)
    expect(payload.fetch('javascript').first['instability']).to be_between(0.0, 1.0).inclusive
    expect(payload.fetch('javascript').first['complexity']).to be > 0
    language_by_path = payload.fetch('javascript').to_h { |file| [file['path'], file['language']] }
    expect(language_by_path['src/b.ts']).to eq('typescript')
    expect(language_by_path['src/leaf.tsx']).to eq('typescript')
    expect(payload.fetch('javascript').map { |file| file['complexity'] }).to include(be > 0)
    expect(payload.fetch('javascript').map { |file| file['score'] }).to all(be_between(0.0, 1.0).inclusive)
  end
end
