# frozen_string_literal: true

require 'spec_helper'
require 'stud_finder/fan_in'

RSpec.describe 'Ruby reference fact reuse' do
  let(:root) { Dir.mktmpdir('reference-cache') }
  let(:repo) { File.join(root, 'repo') }
  let(:cache) { File.join(root, 'cache') }

  after { FileUtils.remove_entry(root) }

  def write(path, text)
    FileUtils.mkdir_p(File.dirname(File.join(repo, path)))
    File.write(File.join(repo, path), text)
  end

  def analyzer(cached: true, rails: true, path: repo)
    files = Dir.chdir(path) { Dir['**/*.rb'].sort }
    StudFinder::FanIn.new(repo_path: path, files: files, cache_dir: cached ? cache : nil,
                          rails_inference: rails, stderr: StringIO.new)
  end

  def expect_fresh_parity(instance = analyzer)
    result = instance.call
    expect(result.to_h).to eq(analyzer(cached: false).call.to_h)
    result
  end

  before do
    write('app/models/user.rb', 'class User; end')
    write('app/services/client.rb', 'class Client; User.new; Missing.new; end')
  end

  it 'reuses persisted facts without invoking the parser, including after relocation' do
    expected = analyzer.call
    moved = File.join(root, 'moved')
    FileUtils.cp_r(repo, moved)
    warm = analyzer(path: moved)
    expect(RuboCop::ProcessedSource).not_to receive(:new)
    expect(warm.call.to_h).to eq(expected.to_h)
    expect(warm.cache_stats).to eq(hits: 2, misses: 0)
  end

  it 'invalidates changed contents even when size and modification time are unchanged' do
    analyzer.call
    path = File.join(repo, 'app/services/client.rb')
    timestamp = File.mtime(path)
    write('app/services/client.rb', 'class Client; Nope.new; Missing.new; end')
    File.utime(timestamp, timestamp, path)
    warm = analyzer
    expect(expect_fresh_parity(warm).counts['app/models/user.rb']).to eq(0)
    expect(warm.cache_stats).to eq(hits: 1, misses: 1)
  end

  it 're-resolves cached unresolved references when a target is added or removed' do
    analyzer.call
    write('app/models/missing.rb', 'class Missing; end')
    warm = analyzer
    expect(expect_fresh_parity(warm).counts['app/models/missing.rb']).to eq(1)
    expect(warm.cache_stats).to eq(hits: 2, misses: 1)
    File.delete(File.join(repo, 'app/models/user.rb'))
    result = expect_fresh_parity
    expect(result.edges['app/services/client.rb'][:dependencies]).to eq(['app/models/missing.rb'])
  end

  it 're-resolves unchanged callers when a new namespace shadows a qualified reference' do
    write('config/shared.rb', 'class Shared::Thing; end')
    write('app/services/admin/client.rb', 'module Admin; class Client; Shared::Thing.new; end; end')
    analyzer.call
    write('app/models/admin/shared.rb', 'module Admin; module Shared; end; end')
    result = expect_fresh_parity
    expect(result.edges['app/services/admin/client.rb'][:dependencies]).to be_empty
  end

  it 'invalidates path-derived ownership on rename' do
    analyzer.call
    File.rename(File.join(repo, 'app/models/user.rb'), File.join(repo, 'app/models/account.rb'))
    expect(expect_fresh_parity.counts['app/models/account.rb']).to eq(0)
  end

  it 'separates Rails inference settings' do
    write('app/models/post.rb', 'class Post; belongs_to :user; end')
    analyzer.call
    disabled = analyzer(rails: false)
    expect(disabled.call.to_h).to eq(analyzer(cached: false, rails: false).call.to_h)
    expect(disabled.cache_stats[:hits]).to eq(0)
  end

  it 'does not retain stale in-memory ASTs when the same analyzer is called again' do
    instance = analyzer
    instance.call
    write('app/services/client.rb', 'class Client; end')
    expect_fresh_parity(instance)
    expect(instance.cache_stats).to eq(hits: 1, misses: 1)
  end

  it 'preserves duplicate constant ownership and self-reference counting' do
    write('config/user.rb', 'class User; User.new; end')
    result = expect_fresh_parity
    expect(result.counts.values_at('app/models/user.rb', 'config/user.rb')).to eq([2, 2])
    expect_fresh_parity
  end

  it 'falls back after truncated or structurally invalid cache data' do
    expected = analyzer.call.to_h
    cache_file = Dir[File.join(cache, '*.json')].first
    ['{', 'null', '{"schema":1,"entries":[]}'].each do |bad|
      File.write(cache_file, bad)
      expect(analyzer.call.to_h).to eq(expected)
    end
    payload = JSON.parse(File.read(cache_file))
    payload['entries'].each_value { |entry| entry['value']['references'] = [nil] }
    File.write(cache_file, JSON.generate(payload))
    expect(analyzer.call.to_h).to eq(expected)
  end

  it 'retries extraction failures instead of caching incomplete reference sets' do
    broken = analyzer
    allow(broken).to receive(:lexical_namespace).and_raise(StandardError, 'temporary failure')
    expect(broken.call.warnings).to include('fan_in_reference_resolution_failed')
    recovered = analyzer
    expect(expect_fresh_parity(recovered).warnings).to be_empty
    expect(recovered.cache_stats[:hits]).to eq(0)
  end

  it 'parses files needing fallback ownership only once' do
    write('config/settings.rb', 'class Settings; User.new; end')
    expect(RuboCop::ProcessedSource).to receive(:new).exactly(3).times.and_call_original
    analyzer(cached: false).call
  end
end
