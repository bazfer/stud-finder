# frozen_string_literal: true

require_relative 'lib/stud_finder/version'

Gem::Specification.new do |spec|
  spec.name = 'stud-finder'
  spec.version = StudFinder::VERSION
  spec.authors = ['bazfer']
  spec.email = ['']

  spec.summary = 'Find high-risk Ruby files in Rails codebases.'
  spec.description = 'A Ruby CLI that analyzes Rails codebases and ranks files by risk.'
  spec.homepage = 'https://github.com/bazfer/stud-finder'
  spec.license = 'MIT'
  spec.required_ruby_version = '>= 3.1'

  spec.metadata['homepage_uri'] = spec.homepage
  spec.metadata['source_code_uri'] = spec.homepage
  spec.metadata['rubygems_mfa_required'] = 'true'

  spec.files = Dir.chdir(__dir__) do
    Dir['bin/*', 'lib/**/*.rb', 'README.md', 'TRD.md', 'LICENSE*']
  end
  spec.bindir = 'bin'
  spec.executables = ['stud-finder']
  spec.require_paths = ['lib']

  spec.add_dependency 'rexml'
  spec.add_dependency 'rubocop', '>= 1.0'
  spec.add_dependency 'rubocop-ast', '>= 1.0'

  spec.add_development_dependency 'rspec', '>= 3.12'
  spec.add_development_dependency 'simplecov', '>= 0.22'
end
