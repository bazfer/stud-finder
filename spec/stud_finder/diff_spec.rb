# frozen_string_literal: true

require 'spec_helper'
require 'stud_finder/diff'

RSpec.describe StudFinder::Diff do
  around do |example|
    Dir.mktmpdir('stud-finder-diff') do |dir|
      @repo = dir
      example.run
    end
  end

  def git(*args)
    system('git', '-C', @repo, *args, out: File::NULL, err: File::NULL)
  end

  def write(relative, content)
    path = File.join(@repo, relative)
    FileUtils.mkdir_p(File.dirname(path))
    File.write(path, content)
  end

  before do
    system('git', 'init', '-q', @repo)
    git('config', 'user.email', 'stud-finder@example.test')
    git('config', 'user.name', 'Stud Finder')
    write('app/models/user.rb', "class User\nend\n")
    write('app/models/post.rb', "class Post\nend\n")
    git('add', '.')
    git('commit', '-qm', 'base')
    git('branch', 'base') # ref to diff the feature work against

    write('app/models/post.rb', "class Post\n  def edit; end\nend\n")
    write('app/services/new_service.rb', "class NewService\nend\n")
    git('add', '.')
    git('commit', '-qm', 'feature')
  end

  it 'returns the files changed on HEAD since the merge-base with the base ref' do
    paths = described_class.new(repo_path: @repo, base_ref: 'base').changed_paths

    expect(paths).to contain_exactly('app/models/post.rb', 'app/services/new_service.rb')
  end

  it 'excludes deleted files from the changed set' do
    File.delete(File.join(@repo, 'app/models/user.rb'))
    git('add', '-A')
    git('commit', '-qm', 'delete user')

    paths = described_class.new(repo_path: @repo, base_ref: 'base').changed_paths

    expect(paths).not_to include('app/models/user.rb')
  end

  it 'returns an empty array when HEAD is identical to the base ref (no changes)' do
    paths = described_class.new(repo_path: @repo, base_ref: 'HEAD').changed_paths

    expect(paths).to eq([])
  end

  it 'raises a clear error when the base ref does not exist' do
    expect do
      described_class.new(repo_path: @repo, base_ref: 'origin/missing-branch').changed_paths
    end.to raise_error(StudFinder::Diff::Error, /diff base ref not found/)
  end

  it 'raises a clear error when git is not found in PATH' do
    allow(Open3).to receive(:capture3).with('git', '-C', @repo, 'rev-parse', '--verify', '--quiet',
                                            'base^{commit}').and_raise(Errno::ENOENT)

    expect do
      described_class.new(repo_path: @repo, base_ref: 'base').changed_paths
    end.to raise_error(StudFinder::Diff::Error, /git not found/)
  end
end
