# frozen_string_literal: true

require 'spec_helper'
require 'stud_finder/loc_counter'

RSpec.describe StudFinder::LocCounter do
  it 'counts non-blank lines without stripping comments' do
    Dir.mktmpdir do |repo|
      FileUtils.mkdir_p(File.join(repo, 'app/models'))
      File.write(File.join(repo, 'app/models/user.rb'), <<~RUBY)
        class User

          # comment counts
          def call
          end
      RUBY

      counts = described_class.new(repo_path: repo, files: ['app/models/user.rb']).call

      expect(counts).to eq('app/models/user.rb' => 4)
    end
  end

  it 'returns zero for files with invalid UTF-8 bytes' do
    Dir.mktmpdir do |repo|
      path = File.join(repo, 'invalid.rb')
      File.binwrite(path, "\xff\xfe not utf-8")

      counts = described_class.new(repo_path: repo, files: ['invalid.rb']).call

      expect(counts).to eq('invalid.rb' => 0)
    end
  end
end
