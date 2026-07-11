# frozen_string_literal: true

require 'spec_helper'
require 'stud_finder/newness'

RSpec.describe StudFinder::Newness do
  let(:now) { Time.utc(2026, 7, 10, 12, 0, 0) }

  def make_repo
    Dir.mktmpdir do |dir|
      system('git', 'init', '-q', dir)
      system('git', '-C', dir, 'config', 'user.email', 'stud-finder@example.test')
      system('git', '-C', dir, 'config', 'user.name', 'Stud Finder')
      yield dir
    end
  end

  def write_file(root, path, content)
    full_path = File.join(root, path)
    FileUtils.mkdir_p(File.dirname(full_path))
    File.write(full_path, content)
  end

  def commit_at(root, message, date)
    system({ 'GIT_AUTHOR_DATE' => date.iso8601, 'GIT_COMMITTER_DATE' => date.iso8601 },
           'git', '-C', root, 'add', '-A')
    system({ 'GIT_AUTHOR_DATE' => date.iso8601, 'GIT_COMMITTER_DATE' => date.iso8601 },
           'git', '-C', root, 'commit', '-qm', message)
  end

  def commit_with_dates(root, message, author_date:, committer_date:)
    system('git', '-C', root, 'add', '-A')
    system({ 'GIT_AUTHOR_DATE' => author_date.iso8601, 'GIT_COMMITTER_DATE' => committer_date.iso8601 },
           'git', '-C', root, 'commit', '-qm', message)
  end

  def metadata(root, files, **options)
    described_class.new(repo_path: root, files: files, now: now, **options).call
  end

  def apply(rows, edges, data)
    described_class.apply(rows: rows, edges: edges, metadata: data).to_h { |row| [row[:path], row] }
  end

  it 'floors a file first committed yesterday from leaf to branch without changing its score' do
    make_repo do |root|
      write_file(root, 'app/models/new_leaf.rb', "class NewLeaf\nend\n")
      commit_at(root, 'add new leaf', now - 86_400)

      rows = [{ path: 'app/models/new_leaf.rb', score: 0.01, classification: 'leaf' }]
      result = apply(rows, { 'app/models/new_leaf.rb' => { dependencies: [] } },
                     metadata(root, ['app/models/new_leaf.rb']))

      expect(result['app/models/new_leaf.rb'][:score]).to eq(0.01)
      expect(result['app/models/new_leaf.rb'][:classification]).to eq('branch')
      expect(result['app/models/new_leaf.rb'][:new_file]).to be(true)
      expect(result['app/models/new_leaf.rb'][:age_days]).to eq(1)
      expect(result['app/models/new_leaf.rb'][:escalation]).to eq('recency_floor')
    end
  end

  it 'uses committer date for file age so recent imports with old authorship still floor' do
    make_repo do |root|
      file = 'app/models/imported_leaf.rb'
      write_file(root, file, "class ImportedLeaf\nend\n")
      commit_with_dates(root, 'import old-authored leaf',
                        author_date: now - (90 * 86_400),
                        committer_date: now - 86_400)

      data = metadata(root, [file])
      result = apply([{ path: file, score: 0.01, classification: 'leaf' }], { file => { dependencies: [] } }, data)

      expect(data[file][:total_commits]).to eq(1)
      expect(data[file][:age_days]).to eq(1)
      expect(result[file][:new_file]).to be(true)
      expect(result[file][:classification]).to eq('branch')
      expect(result[file][:escalation]).to eq('recency_floor')
    end
  end

  it 'escalates a new file that depends on a structural trunk file to trunk before applying the floor' do
    make_repo do |root|
      write_file(root, 'app/models/trunk.rb', "class Trunk\nend\n")
      write_file(root, 'app/models/new_client.rb', "class NewClient < Trunk\nend\n")
      commit_at(root, 'add trunk and new client', now - 86_400)

      rows = [
        { path: 'app/models/trunk.rb', score: 0.9, classification: 'trunk' },
        { path: 'app/models/new_client.rb', score: 0.01, classification: 'leaf' }
      ]
      edges = {
        'app/models/trunk.rb' => { dependencies: [] },
        'app/models/new_client.rb' => { dependencies: ['app/models/trunk.rb'] }
      }
      result = apply(rows, edges, metadata(root, %w[app/models/trunk.rb app/models/new_client.rb]))

      expect(result['app/models/new_client.rb'][:classification]).to eq('trunk')
      expect(result['app/models/new_client.rb'][:escalation]).to eq('trunk_adjacent')
    end
  end

  it 'leaves an old low-score file as leaf with no escalation' do
    make_repo do |root|
      file = 'app/models/old_leaf.rb'
      write_file(root, file, "class OldLeaf\nend\n")
      commit_at(root, 'add old leaf', now - (90 * 86_400))
      3.times do |i|
        File.open(File.join(root, file), 'a') { |f| f.puts "# change #{i}" }
        commit_at(root, "touch old leaf #{i}", now - ((80 - i) * 86_400))
      end

      result = apply([{ path: file, score: 0.01, classification: 'leaf' }], { file => { dependencies: [] } },
                     metadata(root, [file]))

      expect(result[file][:classification]).to eq('leaf')
      expect(result[file][:new_file]).to be(false)
      expect(result[file][:escalation]).to eq('')
    end
  end

  it 'decays the recency floor once a file ages past the window and reaches the commit floor' do
    make_repo do |root|
      file = 'app/models/aged_out.rb'
      write_file(root, file, "class AgedOut\nend\n")
      commit_at(root, 'add aged out', now - (31 * 86_400))
      2.times do |i|
        File.open(File.join(root, file), 'a') { |f| f.puts "# change #{i}" }
        commit_at(root, "touch aged out #{i}", now - ((30 - i) * 86_400))
      end

      data = metadata(root, [file], days: 30, min_commits: 3)
      result = apply([{ path: file, score: 0.01, classification: 'leaf' }], { file => { dependencies: [] } }, data)

      expect(data[file][:total_commits]).to eq(3)
      expect(result[file][:new_file]).to be(false)
      expect(result[file][:classification]).to eq('leaf')
      expect(result[file][:escalation]).to eq('')
    end
  end

  it 'treats delete and re-add at the same path as a fresh lineage' do
    make_repo do |root|
      file = 'app/models/reborn.rb'
      write_file(root, file, "class Reborn\nend\n")
      commit_at(root, 'add original reborn', now - (90 * 86_400))
      3.times do |i|
        File.open(File.join(root, file), 'a') { |f| f.puts "# old change #{i}" }
        commit_at(root, "touch original reborn #{i}", now - ((80 - i) * 86_400))
      end
      system('git', '-C', root, 'rm', '-q', file)
      commit_at(root, 'delete original reborn', now - (2 * 86_400))
      write_file(root, file, "class Reborn\n  def fresh = true\nend\n")
      commit_at(root, 're-add reborn', now - 86_400)

      data = metadata(root, [file])
      result = apply([{ path: file, score: 0.01, classification: 'leaf' }], { file => { dependencies: [] } }, data)

      expect(data[file][:total_commits]).to eq(1)
      expect(result[file][:new_file]).to be(true)
      expect(result[file][:age_days]).to eq(1)
      expect(result[file][:classification]).to eq('branch')
      expect(result[file][:escalation]).to eq('recency_floor')
    end
  end

  it 'keeps pre-newness classification behavior when disabled' do
    rows = [{ path: 'app/models/new_leaf.rb', score: 0.01, classification: 'leaf' }]
    data = described_class.disabled_metadata(['app/models/new_leaf.rb'])

    result = apply(rows, { 'app/models/new_leaf.rb' => { dependencies: ['app/models/trunk.rb'] } }, data)

    expect(result['app/models/new_leaf.rb'][:classification]).to eq('leaf')
    expect(result['app/models/new_leaf.rb'][:new_file]).to be(false)
    expect(result['app/models/new_leaf.rb'][:age_days]).to eq(0)
    expect(result['app/models/new_leaf.rb'][:escalation]).to eq('')
  end

  it 'treats delete-add renamed files as new when lineage is ambiguous' do
    make_repo do |root|
      write_file(root, 'app/models/old_name.rb', "class OldName\nend\n")
      commit_at(root, 'add old name', now - (90 * 86_400))

      FileUtils.rm(File.join(root, 'app/models/old_name.rb'))
      write_file(root, 'app/models/new_name.rb', "class NewName\n  def unrelated = true\nend\n")
      commit_at(root, 'replace old name with new name', now - 86_400)

      data = metadata(root, ['app/models/new_name.rb'])
      result = apply([{ path: 'app/models/new_name.rb', score: 0.01, classification: 'leaf' }],
                     { 'app/models/new_name.rb' => { dependencies: [] } }, data)

      expect(result['app/models/new_name.rb'][:new_file]).to be(true)
      expect(result['app/models/new_name.rb'][:classification]).to eq('branch')
      expect(result['app/models/new_name.rb'][:escalation]).to eq('recency_floor')
    end
  end

  it 'rebases git-log paths when scanning a subdirectory' do
    make_repo do |root|
      file = 'app/models/user.rb'
      write_file(root, file, "class User\nend\n")
      commit_at(root, 'add user', now - (90 * 86_400))
      3.times do |i|
        File.open(File.join(root, file), 'a') { |f| f.puts "# change #{i}" }
        commit_at(root, "touch user #{i}", now - ((80 - i) * 86_400))
      end

      data = metadata(File.join(root, 'app'), ['models/user.rb'])

      expect(data['models/user.rb'][:total_commits]).to eq(4)
      expect(data['models/user.rb'][:age_days]).to eq(90)
      expect(data['models/user.rb'][:new_file]).to be(false)
    end
  end

  it 'carries unambiguous git rename history forward to the new path' do
    make_repo do |root|
      old_file = 'app/models/old_name.rb'
      new_file = 'app/models/new_name.rb'
      write_file(root, old_file, "class OldName\nend\n")
      commit_at(root, 'add old name', now - (90 * 86_400))
      3.times do |i|
        File.open(File.join(root, old_file), 'a') { |f| f.puts "# change #{i}" }
        commit_at(root, "touch old name #{i}", now - ((80 - i) * 86_400))
      end
      system('git', '-C', root, 'mv', old_file, new_file)
      commit_at(root, 'rename old name to new name', now - 86_400)

      data = metadata(root, [new_file])
      result = apply([{ path: new_file, score: 0.01, classification: 'leaf' }],
                     { new_file => { dependencies: [] } }, data)

      expect(data[new_file][:total_commits]).to eq(5)
      expect(data[new_file][:age_days]).to eq(90)
      expect(result[new_file][:new_file]).to be(false)
      expect(result[new_file][:classification]).to eq('leaf')
      expect(result[new_file][:escalation]).to eq('')
    end
  end

  it 'carries in-subtree rename history when scanning a subdirectory' do
    make_repo do |root|
      old_file = 'app/models/customer.rb'
      new_file = 'app/models/user.rb'
      wanted_file = 'models/user.rb'
      write_file(root, old_file, "class Customer\nend\n")
      commit_at(root, 'add customer', now - (90 * 86_400))
      3.times do |i|
        File.open(File.join(root, old_file), 'a') { |f| f.puts "# customer change #{i}" }
        commit_at(root, "touch customer #{i}", now - ((80 - i) * 86_400))
      end
      system('git', '-C', root, 'mv', old_file, new_file)
      commit_at(root, 'rename customer to user', now - 86_400)

      data = metadata(File.join(root, 'app'), [wanted_file])
      result = apply([{ path: wanted_file, score: 0.01, classification: 'leaf' }],
                     { wanted_file => { dependencies: [] } }, data)

      expect(data[wanted_file][:total_commits]).to eq(5)
      expect(data[wanted_file][:age_days]).to eq(90)
      expect(result[wanted_file][:new_file]).to be(false)
      expect(result[wanted_file][:classification]).to eq('leaf')
      expect(result[wanted_file][:escalation]).to eq('')
    end
  end

  it 'carries rename history from outside the analysis subtree into a subdirectory scan' do
    make_repo do |root|
      old_file = 'lib/foo.rb'
      new_file = 'models/foo.rb'
      write_file(root, old_file, "class Foo\nend\n")
      commit_at(root, 'add foo outside app', now - (90 * 86_400))
      3.times do |i|
        File.open(File.join(root, old_file), 'a') { |f| f.puts "# change #{i}" }
        commit_at(root, "touch foo outside app #{i}", now - ((80 - i) * 86_400))
      end
      FileUtils.mkdir_p(File.join(root, 'app/models'))
      system('git', '-C', root, 'mv', old_file, 'app/models/foo.rb')
      commit_at(root, 'move foo into app models', now - 86_400)

      data = metadata(File.join(root, 'app'), [new_file])
      result = apply([{ path: new_file, score: 0.01, classification: 'leaf' }],
                     { new_file => { dependencies: [] } }, data)

      expect(data[new_file][:total_commits]).to eq(5)
      expect(data[new_file][:age_days]).to be_between(89, 91).inclusive
      expect(result[new_file][:new_file]).to be(false)
      expect(result[new_file][:classification]).to eq('leaf')
      expect(result[new_file][:escalation]).to eq('')
    end
  end

  it 'does not let an unrelated prior delete at the target path block renamed lineage history' do
    make_repo do |root|
      target_file = 'app/models/user.rb'
      source_file = 'app/models/customer.rb'
      write_file(root, target_file, "class User\nend\n")
      commit_at(root, 'add unrelated user', now - (100 * 86_400))
      File.open(File.join(root, target_file), 'a') { |f| f.puts '# unrelated change' }
      commit_at(root, 'touch unrelated user', now - (99 * 86_400))
      write_file(root, source_file, "class Customer\nend\n")
      commit_at(root, 'add customer', now - (90 * 86_400))
      [80, 70, 65].each_with_index do |days_ago, i|
        File.open(File.join(root, source_file), 'a') { |f| f.puts "# customer change #{i}" }
        commit_at(root, "touch customer #{i}", now - (days_ago * 86_400))
      end
      system('git', '-C', root, 'rm', '-q', target_file)
      commit_at(root, 'delete unrelated user', now - (60 * 86_400))
      system('git', '-C', root, 'mv', source_file, target_file)
      commit_at(root, 'rename customer to user', now - 86_400)

      data = metadata(root, [target_file])
      result = apply([{ path: target_file, score: 0.01, classification: 'leaf' }],
                     { target_file => { dependencies: [] } }, data)

      expect(data[target_file][:total_commits]).to eq(5)
      expect(data[target_file][:age_days]).to be_between(89, 91).inclusive
      expect(result[target_file][:new_file]).to be(false)
      expect(result[target_file][:classification]).to eq('leaf')
      expect(result[target_file][:escalation]).to eq('')
    end
  end

  it 'ignores unrelated outside-subtree history that collides with a rebased inside path' do
    make_repo do |root|
      write_file(root, 'models/user.rb', "class OutsideUser\nend\n")
      commit_at(root, 'add outside user', now - (90 * 86_400))
      3.times do |i|
        File.open(File.join(root, 'models/user.rb'), 'a') { |f| f.puts "# outside change #{i}" }
        commit_at(root, "touch outside user #{i}", now - ((80 - i) * 86_400))
      end
      write_file(root, 'app/models/user.rb', "class AppUser\nend\n")
      commit_at(root, 'add app user', now - 86_400)

      data = metadata(File.join(root, 'app'), ['models/user.rb'])

      expect(data['models/user.rb'][:total_commits]).to eq(1)
      expect(data['models/user.rb'][:age_days]).to eq(1)
      expect(data['models/user.rb'][:new_file]).to be(true)
    end
  end

  it 'keeps a new sibling-subtree file new when an older root file has the same rebased key' do
    make_repo do |root|
      write_file(root, 'lib/foo.rb', "class RootFoo\nend\n")
      commit_at(root, 'add root foo', now - (90 * 86_400))
      3.times do |i|
        File.open(File.join(root, 'lib/foo.rb'), 'a') { |f| f.puts "# root change #{i}" }
        commit_at(root, "touch root foo #{i}", now - ((80 - i) * 86_400))
      end
      write_file(root, 'app/lib/foo.rb', "class AppFoo\nend\n")
      commit_at(root, 'add app foo', now - 86_400)

      data = metadata(File.join(root, 'app'), ['lib/foo.rb'])

      expect(data['lib/foo.rb'][:total_commits]).to eq(1)
      expect(data['lib/foo.rb'][:age_days]).to eq(1)
      expect(data['lib/foo.rb'][:new_file]).to be(true)
    end
  end

  it 'preserves mature outside-to-inside rename lineage when scanning a subdirectory' do
    make_repo do |root|
      old_file = 'legacy/user.rb'
      new_file = 'app/models/user.rb'
      write_file(root, old_file, "class LegacyUser\nend\n")
      commit_at(root, 'add legacy user', now - (90 * 86_400))
      3.times do |i|
        File.open(File.join(root, old_file), 'a') { |f| f.puts "# legacy change #{i}" }
        commit_at(root, "touch legacy user #{i}", now - ((80 - i) * 86_400))
      end
      FileUtils.mkdir_p(File.join(root, 'app/models'))
      system('git', '-C', root, 'mv', old_file, new_file)
      commit_at(root, 'move legacy user into app', now - 86_400)

      data = metadata(File.join(root, 'app'), ['models/user.rb'])
      result = apply([{ path: 'models/user.rb', score: 0.01, classification: 'leaf' }],
                     { 'models/user.rb' => { dependencies: [] } }, data)

      expect(data['models/user.rb'][:total_commits]).to eq(5)
      expect(data['models/user.rb'][:age_days]).to eq(90)
      expect(result['models/user.rb'][:new_file]).to be(false)
      expect(result['models/user.rb'][:classification]).to eq('leaf')
      expect(result['models/user.rb'][:escalation]).to eq('')
    end
  end

  it 'returns disabled metadata when called directly against a shallow repository' do
    make_repo do |root|
      file = 'app/models/user.rb'
      write_file(root, file, "class User\nend\n")
      commit_at(root, 'add user', now - (90 * 86_400))
      File.open(File.join(root, file), 'a') { |f| f.puts '# latest shallow-only change' }
      commit_at(root, 'touch user', now - 86_400)

      Dir.mktmpdir do |clone_parent|
        shallow_root = File.join(clone_parent, 'shallow')
        system('git', 'clone', '-q', '--depth', '1', "file://#{root}", shallow_root)

        data = described_class.new(repo_path: shallow_root, files: [file], now: now).call

        expect(data[file]).to eq(new_file: false, age_days: 0, total_commits: 0, escalation: '')
      end
    end
  end
end
