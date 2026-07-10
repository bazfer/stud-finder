# frozen_string_literal: true

require 'spec_helper'
require 'tempfile'
require 'stud_finder/coverage/cobertura'

RSpec.describe StudFinder::Coverage::Cobertura do
  def write_report(xml)
    file = Tempfile.new(['coverage', '.xml'])
    file.write(xml)
    file.close
    file.path
  end

  def parse(xml, files: %w[app/models/user.rb app/models/post.rb])
    path = write_report(xml)
    described_class.new(path: path, files: files).call
  ensure
    FileUtils.rm_f(path) if path
  end

  it 'parses line-rate correctly as a fraction' do
    coverage = parse(<<~XML)
      <coverage>
        <packages>
          <package>
            <classes>
              <class filename="app/models/user.rb" line-rate="0.95" />
            </classes>
          </package>
        </packages>
      </coverage>
    XML

    expect(coverage['app/models/user.rb']).to eq(0.95)
  end

  it 'returns fractions rather than percentages' do
    coverage = parse(<<~XML, files: ['app/models/user.rb'])
      <coverage><packages><package><classes>
        <class filename="app/models/user.rb" line-rate="0.5" />
      </classes></package></packages></coverage>
    XML

    expect(coverage['app/models/user.rb']).to eq(0.5)
  end

  it 'omits files absent from the report so missingness reaches the scorer' do
    coverage = parse(<<~XML)
      <coverage><packages><package><classes>
        <class filename="app/models/user.rb" line-rate="0.75" />
      </classes></package></packages></coverage>
    XML

    expect(coverage).not_to have_key('app/models/post.rb')
  end

  it 'handles multiple packages and classes' do
    coverage = parse(<<~XML, files: %w[app/models/user.rb app/models/post.rb app/services/auth_service.rb])
      <coverage>
        <packages>
          <package name="models">
            <classes>
              <class filename="app/models/user.rb" line-rate="1.0" />
              <class filename="app/models/post.rb" line-rate="0.25" />
            </classes>
          </package>
          <package name="services">
            <classes>
              <class filename="app/services/auth_service.rb" line-rate="0.8" />
            </classes>
          </package>
        </packages>
      </coverage>
    XML

    expect(coverage).to eq(
      'app/models/user.rb' => 1.0,
      'app/models/post.rb' => 0.25,
      'app/services/auth_service.rb' => 0.8
    )
  end

  it 'raises a descriptive error for malformed XML' do
    path = write_report('<coverage><packages>')
    parser = described_class.new(path: path, files: ['app/models/user.rb'])

    expect { parser.call }.to raise_error(StudFinder::Coverage::Cobertura::Error, /malformed coverage XML/)
  ensure
    FileUtils.rm_f(path) if path
  end
end
