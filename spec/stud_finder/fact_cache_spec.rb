# frozen_string_literal: true

require 'spec_helper'
require 'stud_finder/fact_cache'

RSpec.describe StudFinder::FactCache do
  let(:directory) { Dir.mktmpdir('fact-cache') }
  let(:stderr) { StringIO.new }
  let(:cache) { described_class.new(directory: directory, namespace: 'test', stderr: stderr) }

  after { FileUtils.remove_entry(directory) }

  it 'persists matching fingerprints and removes deleted entries' do
    cache.store('a.rb', 'a', { 'constant' => 'A' })
    cache.store('b.rb', 'b', {})
    cache.retain(['a.rb'])
    cache.save
    restored = described_class.new(directory: directory, namespace: 'test')
    expect(restored.fetch('a.rb', 'a')).to eq('constant' => 'A')
    expect(restored.fetch('a.rb', 'changed')).to be_nil
    expect(restored.fetch('b.rb', 'b')).to be_nil
    expect(Dir[File.join(directory, '*.tmp')]).to be_empty
  end

  it 'keeps incompatible namespaces separate' do
    cache.store('a.rb', 'a', {})
    cache.save
    other = described_class.new(directory: directory, namespace: 'other')
    expect(other.fetch('a.rb', 'a')).to be_nil
  end

  it 'does not rewrite a cache with no changes' do
    expect(FileUtils).not_to receive(:mkdir_p)
    cache.save
  end

  it 'keeps a previous snapshot readable when replacement fails' do
    cache.store('a.rb', 'a', { 'old' => true })
    cache.save
    cache.store('a.rb', 'new', {})
    allow(File).to receive(:rename).and_raise(Errno::EACCES)
    expect { cache.save }.not_to raise_error
    restored = described_class.new(directory: directory, namespace: 'test')
    expect(restored.fetch('a.rb', 'a')).to eq('old' => true)
    expect(stderr.string).to include('fact cache write failed')
  end
end
