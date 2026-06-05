# frozen_string_literal: true

require 'spec_helper'
require 'stud_finder/temporal_coupling'

RSpec.describe StudFinder::TemporalCoupling do
  let(:repo_path) { '/fake/repo' }
  let(:files) { %w[app/models/user.rb app/models/role.rb app/services/auth.rb app/controllers/users_controller.rb] }

  def make_coupling(git_output, files: self.files, min_co_changes: 2, coupling_threshold: 0.30)
    tc = described_class.new(
      repo_path: repo_path, files: files, days: 90,
      min_co_changes: min_co_changes, coupling_threshold: coupling_threshold
    )
    allow(tc).to receive(:git_log).and_return([git_output, '', double(success?: true)])
    tc.call
  end

  def git_log_output(*commits)
    commits.each_with_index.map do |file_list, i|
      sha = format('%040d', i + 1)
      "#{sha}\n\n#{file_list.join("\n")}\n"
    end.join("\n")
  end

  it 'returns empty pairs when no commits share files' do
    output = git_log_output(
      ['app/models/user.rb'],
      ['app/models/role.rb']
    )
    result = make_coupling(output)
    expect(result.pairs).to be_empty
  end

  it 'counts co-changes correctly' do
    output = git_log_output(
      ['app/models/user.rb', 'app/models/role.rb'],
      ['app/models/user.rb', 'app/models/role.rb'],
      ['app/models/user.rb', 'app/models/role.rb']
    )
    result = make_coupling(output, min_co_changes: 2)
    expect(result.pairs['app/models/user.rb']).not_to be_nil
    partner = result.pairs['app/models/user.rb'].find { |p| p[:path] == 'app/models/role.rb' }
    expect(partner[:co_changes]).to eq(3)
  end

  it 'computes coupling as co_changes / min(own_changes_A, own_changes_B)' do
    # user.rb appears 4 times, role.rb appears 3 times, they co-change 3 times
    # coupling = 3 / min(4, 3) = 3/3 = 1.0
    output = git_log_output(
      ['app/models/user.rb', 'app/models/role.rb'],
      ['app/models/user.rb', 'app/models/role.rb'],
      ['app/models/user.rb', 'app/models/role.rb'],
      ['app/models/user.rb']
    )
    result = make_coupling(output, min_co_changes: 2, coupling_threshold: 0.0)
    partner = result.pairs['app/models/user.rb'].find { |p| p[:path] == 'app/models/role.rb' }
    expect(partner[:coupling]).to eq(1.0)
  end

  it 'suppresses pairs below min_co_changes' do
    output = git_log_output(
      ['app/models/user.rb', 'app/models/role.rb'],
      ['app/models/user.rb', 'app/models/role.rb']
    )
    result = make_coupling(output, min_co_changes: 5)
    expect(result.pairs).to be_empty
  end

  it 'suppresses pairs below coupling_threshold' do
    # co_change 2 times, user changes 10 times (separate commits) => coupling = 2/2 = 1.0
    # But let's force a low coupling: co_change 2, user changes 10, role changes 2
    # coupling = 2 / min(10, 2) = 2/2 = 1.0 — that's still high
    # To get low coupling: co_change 2 times, but one file changes 20 times total
    # Use min_co_changes: 2, so the pair passes that gate
    # co_change=2, own_A=20, own_B=2 => coupling=2/2=1.0 (min is 2)
    # Actually the denominator is min(own_A, own_B), so to get low coupling we need large min
    # Let's use: co_change=2, own_A=2, own_B=10 => coupling=2/2=1.0 — still high
    # To get coupling < threshold: co_change=2, both files change 10 times => 2/10 = 0.2 < 0.30
    commits = []
    10.times { commits << ['app/models/user.rb'] }
    10.times { commits << ['app/models/role.rb'] }
    2.times  { commits << ['app/models/user.rb', 'app/models/role.rb'] }
    output = git_log_output(*commits)
    result = make_coupling(output, min_co_changes: 2, coupling_threshold: 0.30)
    # coupling = 2 / min(12, 12) = 2/12 ≈ 0.1667 — below 0.30
    expect(result.pairs).to be_empty
  end

  it 'sorts partners by coupling descending' do
    # user.rb co-changes with role.rb 5 times (out of 5) and auth.rb 3 times (out of 5)
    # role.rb own=5, auth.rb own=4
    # coupling(user,role) = 5/min(5,5) = 1.0
    # coupling(user,auth) = 3/min(5,4) = 3/4 = 0.75
    output = git_log_output(
      ['app/models/user.rb', 'app/models/role.rb', 'app/services/auth.rb'],
      ['app/models/user.rb', 'app/models/role.rb', 'app/services/auth.rb'],
      ['app/models/user.rb', 'app/models/role.rb', 'app/services/auth.rb'],
      ['app/models/user.rb', 'app/models/role.rb'],
      ['app/models/user.rb', 'app/models/role.rb']
    )
    result = make_coupling(output, min_co_changes: 2, coupling_threshold: 0.0)
    partners = result.pairs['app/models/user.rb']
    expect(partners).not_to be_nil
    couplings = partners.map { |p| p[:coupling] }
    expect(couplings).to eq(couplings.sort.reverse)
  end

  it 'ignores files not in the scored set' do
    output = git_log_output(
      ['app/models/user.rb', 'some/external/gem/file.rb']
    )
    result = make_coupling(output, min_co_changes: 1, coupling_threshold: 0.0)
    expect(result.pairs).to be_empty
  end

  it 'returns empty pairs when git fails' do
    tc = described_class.new(repo_path: repo_path, files: files, days: 90)
    allow(tc).to receive(:git_log).and_return(['', '', double(success?: false)])
    result = tc.call
    expect(result.pairs).to be_empty
    expect(result.warnings).to include('git_error')
  end

  it 'populates pairs symmetrically — both files see each other as partners' do
    output = git_log_output(
      ['app/models/user.rb', 'app/models/role.rb'],
      ['app/models/user.rb', 'app/models/role.rb'],
      ['app/models/user.rb', 'app/models/role.rb']
    )
    result = make_coupling(output, min_co_changes: 2, coupling_threshold: 0.0)
    expect(result.pairs['app/models/user.rb'].map { |p| p[:path] }).to include('app/models/role.rb')
    expect(result.pairs['app/models/role.rb'].map { |p| p[:path] }).to include('app/models/user.rb')
  end

  it 'does not double-count a file listed twice in a single commit' do
    output = git_log_output(
      ['app/models/user.rb', 'app/models/user.rb', 'app/models/role.rb'], # user.rb duplicated
      ['app/models/user.rb', 'app/models/role.rb']
    )
    result = make_coupling(output, min_co_changes: 1, coupling_threshold: 0.0)
    partner = result.pairs['app/models/user.rb'].find { |p| p[:path] == 'app/models/role.rb' }

    expect(partner[:co_changes]).to eq(2) # two commits, not inflated to 3 by the dup
    expect(result.pairs['app/models/user.rb'].map { |p| p[:path] }).not_to include('app/models/user.rb')
  end
end
