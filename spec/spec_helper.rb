# frozen_string_literal: true

require 'simplecov'
SimpleCov.start do
  enable_coverage :branch
  minimum_coverage line: 90, branch: 75
  add_filter '/spec/'
end

require 'tmpdir'
require 'fileutils'
require 'stringio'

$LOAD_PATH.unshift File.expand_path('../lib', __dir__)

RSpec.configure do |config|
  config.expect_with :rspec do |expectations|
    expectations.syntax = :expect
  end
end
