# frozen_string_literal: true

require 'spec_helper'
require 'tempfile'
require 'stud_finder/coverage/lcov'

RSpec.describe StudFinder::Coverage::Lcov do
  def write_report(content)
    file = Tempfile.new(['coverage', '.info'])
    file.write(content)
    file.close
    file.path
  end

  def parse(content, files: %w[app/models/user.rb app/models/post.rb], project_root: nil)
    path = write_report(content)
    described_class.new(path: path, files: files, project_root: project_root).call
  ensure
    FileUtils.rm_f(path) if path
  end

  it 'parses LF and LH fields as a fraction' do
    coverage = parse(<<~LCOV, files: ['app/models/user.rb'])
      TN:
      SF:app/models/user.rb
      LF:10
      LH:8
      end_of_record
    LCOV

    expect(coverage['app/models/user.rb']).to eq(0.8)
  end

  it 'falls back to DA line hits when summary fields are absent' do
    coverage = parse(<<~LCOV, files: ['app/models/user.rb'])
      SF:app/models/user.rb
      DA:1,1
      DA:2,0
      DA:3,4
      end_of_record
    LCOV

    expect(coverage['app/models/user.rb']).to eq(2.0 / 3.0)
  end

  it 'omits absent files so missingness reaches the scorer' do
    coverage = parse(<<~LCOV)
      SF:app/models/user.rb
      LF:4
      LH:3
      end_of_record
    LCOV

    expect(coverage['app/models/user.rb']).to eq(0.75)
    expect(coverage).not_to have_key('app/models/post.rb')
  end

  it 'returns 0.0 when a record has no DA lines' do
    coverage = parse("SF:app/models/empty.rb\nend_of_record\n", files: ['app/models/empty.rb'])

    expect(coverage['app/models/empty.rb']).to eq(0.0)
  end

  it 'matches relative SF paths directly' do
    coverage = parse(<<~LCOV, files: ['app/javascript/components/Foo.jsx'])
      SF:app/javascript/components/Foo.jsx
      LF:4
      LH:1
      end_of_record
    LCOV

    expect(coverage['app/javascript/components/Foo.jsx']).to eq(0.25)
  end

  it 'maps absolute SF paths from another machine by walking suffixes' do
    coverage = parse(<<~LCOV, files: ['app/models/user.rb'], project_root: '/home/fernando/Projects/covalent-ojt')
      SF:/Users/fernandobaz/Desktop/covalent-ojt/app/models/user.rb
      LF:3
      LH:2
      end_of_record
    LCOV

    expect(coverage['app/models/user.rb']).to eq(2.0 / 3.0)
  end

  it 'uses the most specific suffix when absolute SF paths are ambiguous' do
    coverage = parse(
      <<~LCOV,
        SF:/Users/fernandobaz/Desktop/covalent-ojt/app/models/user.rb
        LF:3
        LH:2
        end_of_record
      LCOV
      files: ['user.rb', 'app/models/user.rb'],
      project_root: '/home/fernando/Projects/covalent-ojt'
    )

    expect(coverage['app/models/user.rb']).to eq(2.0 / 3.0)
    expect(coverage).not_to have_key('user.rb')
  end

  it 'leaves unmatched absolute SF paths safely unmapped' do
    coverage = parse(<<~LCOV, files: ['app/models/user.rb'])
      SF:/Users/fernandobaz/Desktop/other-app/lib/tasks/report.rb
      LF:2
      LH:2
      end_of_record
    LCOV

    expect(coverage).not_to have_key('app/models/user.rb')
  end

  it 'strips the target project root from absolute SF paths' do
    Dir.mktmpdir do |root|
      coverage = parse(<<~LCOV, files: ['src/foo.js'], project_root: root)
        SF:#{root}/src/foo.js
        LF:8
        LH:6
        end_of_record
      LCOV

      expect(coverage['src/foo.js']).to eq(0.75)
    end
  end
end
