# frozen_string_literal: true

require 'spec_helper'
require 'stud_finder/edges'

RSpec.describe StudFinder::Edges do
  def row(path, score:, fan_in:, fan_out:)
    instability = (fan_in + fan_out).zero? ? 0.0 : (fan_out.to_f / (fan_in + fan_out)).round(4)
    { path: path, score: score, classification: 'leaf', fan_in: fan_in, fan_out: fan_out, instability: instability }
  end

  let(:rows) do
    [
      row('app/models/user.rb',     score: 0.9, fan_in: 10, fan_out: 1),
      row('app/models/role.rb',     score: 0.6, fan_in: 5,  fan_out: 0),
      row('app/services/greet.rb',  score: 0.3, fan_in: 0,  fan_out: 2)
    ]
  end

  let(:edges) do
    {
      'app/models/user.rb' => { dependents: ['app/services/greet.rb'], dependencies: ['app/models/role.rb'] },
      'app/models/role.rb' => { dependents: ['app/models/user.rb', 'app/services/greet.rb'], dependencies: [] },
      'app/services/greet.rb' => { dependents: [], dependencies: ['app/models/user.rb', 'app/models/role.rb'] }
    }
  end

  def edges_output(target)
    stdout = StringIO.new
    stderr = StringIO.new
    status = described_class.new(target: target, rows: rows, edges: edges,
                                 stdout: stdout, stderr: stderr).call
    [status, stdout.string, stderr.string]
  end

  it 'emits dependents and dependencies for a known file' do
    status, stdout, _stderr = edges_output('app/models/user.rb')

    expect(status).to eq(0)
    expect(stdout).to include('app/models/user.rb')
    expect(stdout).to include('Dependents')
    expect(stdout).to include('app/services/greet.rb')
    expect(stdout).to include('Dependencies')
    expect(stdout).to include('app/models/role.rb')
  end

  it 'sorts dependents by score descending' do
    status, stdout, _stderr = edges_output('app/models/role.rb')

    expect(status).to eq(0)
    user_pos  = stdout.index('app/models/user.rb')
    greet_pos = stdout.index('app/services/greet.rb')
    expect(user_pos).to be < greet_pos
  end

  it 'emits header with score, class, fan_in, fan_out, and instability' do
    _status, stdout, _stderr = edges_output('app/models/user.rb')

    expect(stdout).to include('score: 0.9000')
    expect(stdout).to include('fan_in: 10')
    expect(stdout).to include('fan_out: 1')
    expect(stdout).to include('instability:')
  end

  it 'returns 1 and emits an error for a file not in the scored set' do
    status, _stdout, stderr = edges_output('app/models/missing.rb')

    expect(status).to eq(1)
    expect(stderr).to include("'app/models/missing.rb' was not found")
  end

  it 'returns 1 and prints usage when no target is given' do
    stderr = StringIO.new
    status = described_class.new(target: nil, rows: rows, edges: edges,
                                 stdout: StringIO.new, stderr: stderr).call

    expect(status).to eq(1)
    expect(stderr.string).to include('Usage: stud-finder edges FILE [PATH]')
  end

  it 'shows (none in scored file set) when all edges are outside the scored set' do
    sparse_edges = {
      'app/models/user.rb' => { dependents: ['external/gem.rb'], dependencies: [] }
    }
    stdout = StringIO.new
    described_class.new(target: 'app/models/user.rb', rows: rows, edges: sparse_edges,
                        stdout: stdout, stderr: StringIO.new).call

    expect(stdout.string).to include('(none in scored file set)')
  end
end
