# frozen_string_literal: true

require 'json'
require 'spec_helper'
require 'tempfile'
require 'stud_finder/coverage/resultset'

RSpec.describe StudFinder::Coverage::Resultset do
  def write_report(payload)
    file = Tempfile.new(['resultset', '.json'])
    file.write(JSON.generate(payload))
    file.close
    file.path
  end

  def parse_resultset(payload, files: %w[app/models/user.rb app/models/post.rb], project_root: nil)
    path = write_report(payload)
    described_class.new(path: path, files: files, project_root: project_root).call
  ensure
    FileUtils.rm_f(path) if path
  end

  it 'parses SimpleCov resultset coverage as a fraction' do
    coverage = parse_resultset(
      {
        'RSpec' => {
          'coverage' => {
            'app/models/user.rb' => { 'lines' => [nil, 1, 0, 3] }
          }
        }
      },
      files: ['app/models/user.rb']
    )

    expect(coverage['app/models/user.rb']).to eq(2.0 / 3.0)
  end

  it 'parses top-level coverage payloads' do
    coverage = parse_resultset(
      {
        'coverage' => {
          'app/models/user.rb' => [nil, 1, 1, 0]
        }
      },
      files: ['app/models/user.rb']
    )

    expect(coverage['app/models/user.rb']).to eq(2.0 / 3.0)
  end

  it 'strips the target project root from absolute SimpleCov paths' do
    Dir.mktmpdir do |root|
      coverage = parse_resultset(
        {
          'RSpec' => {
            'coverage' => {
              File.join(root, 'app/models/user.rb') => { 'lines' => [nil, 1, 0, 1] }
            }
          }
        },
        files: ['app/models/user.rb'],
        project_root: root
      )

      expect(coverage['app/models/user.rb']).to eq(2.0 / 3.0)
    end
  end

  it 'maps absolute SimpleCov paths from another machine by walking suffixes' do
    coverage = parse_resultset(
      {
        'RSpec' => {
          'coverage' => {
            '/Users/fernandobaz/Desktop/covalent-ojt/app/models/user.rb' => { 'lines' => [nil, 1, 0, 1] }
          }
        }
      },
      files: ['app/models/user.rb'],
      project_root: '/home/fernando/Projects/covalent-ojt'
    )

    expect(coverage['app/models/user.rb']).to eq(2.0 / 3.0)
  end

  it 'uses the most specific suffix when absolute SimpleCov paths are ambiguous' do
    coverage = parse_resultset(
      {
        'RSpec' => {
          'coverage' => {
            '/Users/fernandobaz/Desktop/covalent-ojt/app/models/user.rb' => { 'lines' => [nil, 1, 0, 1] }
          }
        }
      },
      files: ['user.rb', 'app/models/user.rb'],
      project_root: '/home/fernando/Projects/covalent-ojt'
    )

    expect(coverage['app/models/user.rb']).to eq(2.0 / 3.0)
    expect(coverage).not_to have_key('user.rb')
  end

  it 'leaves unmatched absolute SimpleCov paths safely unmapped' do
    coverage = parse_resultset(
      {
        'RSpec' => {
          'coverage' => {
            '/Users/fernandobaz/Desktop/other-app/lib/tasks/report.rb' => { 'lines' => [1, 1] }
          }
        }
      },
      files: ['app/models/user.rb']
    )

    expect(coverage).not_to have_key('app/models/user.rb')
  end

  it 'omits files absent from the resultset so missingness reaches the scorer' do
    coverage = parse_resultset(
      {
        'RSpec' => {
          'coverage' => {
            'app/models/user.rb' => { 'lines' => [1, 1] }
          }
        }
      }
    )

    expect(coverage).not_to have_key('app/models/post.rb')
  end

  it 'max-merges line hits from multiple suites' do
    coverage = parse_resultset(
      {
        'RSpec' => {
          'coverage' => {
            'app/models/user.rb' => { 'lines' => [3] }
          }
        },
        'Minitest' => {
          'coverage' => {
            'app/models/user.rb' => { 'lines' => [0] }
          }
        }
      },
      files: ['app/models/user.rb']
    )

    expect(coverage['app/models/user.rb']).to eq(1.0)
  end

  it 'keeps trailing executable lines when a later suite has longer line coverage' do
    coverage = parse_resultset(
      {
        'RSpec' => {
          'coverage' => {
            'app/models/user.rb' => { 'lines' => [1] }
          }
        },
        'Minitest' => {
          'coverage' => {
            'app/models/user.rb' => { 'lines' => [0, 0] }
          }
        }
      },
      files: ['app/models/user.rb']
    )

    expect(coverage['app/models/user.rb']).to eq(0.5)
  end

  it 'returns 0.0 when all lines are null' do
    coverage = parse_resultset(
      {
        'RSpec' => {
          'coverage' => {
            'app/models/user.rb' => { 'lines' => [nil, nil] }
          }
        }
      },
      files: ['app/models/user.rb']
    )

    expect(coverage['app/models/user.rb']).to eq(0.0)
  end

  it 'raises a descriptive error for malformed JSON' do
    file = Tempfile.new(['resultset', '.json'])
    file.write('{')
    file.close
    parser = described_class.new(path: file.path, files: ['app/models/user.rb'])

    expect { parser.call }.to raise_error(StudFinder::Coverage::Resultset::Error, /malformed coverage JSON/)
  ensure
    file&.unlink
  end
end
