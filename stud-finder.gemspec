# frozen_string_literal: true

require_relative 'lib/stud_finder/version'

Gem::Specification.new do |spec|
  # CLI-first gem: dash gem name with underscore require path is intentional.
  spec.name = 'stud-finder'
  spec.version = StudFinder::VERSION
  spec.authors = ['bazfer']
  spec.email = ['bazfer@gmail.com']

  spec.summary = 'Rank files by structural risk in Ruby and JavaScript/TypeScript codebases.'
  spec.description = 'A code risk scoring CLI for Ruby and JS/TS projects. Ranks every ' \
                     'file by five signals — fan-in (blast radius), fan-out, cyclomatic ' \
                     'complexity, git churn, and test coverage — with temporal coupling ' \
                     'analysis and a diff mode for scoring only the files changed in a ' \
                     'PR. Table, JSON, CSV, and Markdown output.'
  spec.homepage = 'https://github.com/bazfer/stud-finder'
  spec.license = 'MIT'
  spec.required_ruby_version = '>= 3.2'

  spec.metadata['homepage_uri'] = spec.homepage
  spec.metadata['source_code_uri'] = spec.homepage
  spec.metadata['changelog_uri'] = 'https://github.com/bazfer/stud-finder/blob/main/CHANGELOG.md'
  spec.metadata['bug_tracker_uri'] = 'https://github.com/bazfer/stud-finder/issues'
  spec.metadata['rubygems_mfa_required'] = 'true'

  spec.files = Dir.chdir(__dir__) do
    Dir['bin/*', 'lib/**/*.rb', 'README.md', 'SIGNALS.md', 'CHANGELOG.md', 'LICENSE*']
  end
  spec.bindir = 'bin'
  spec.executables = ['stud-finder']
  spec.require_paths = ['lib']

  spec.add_dependency 'csv', '>= 3.0', '< 4.0'
  spec.add_dependency 'rexml', '>= 3.0', '< 4.0'
  spec.add_dependency 'rubocop', '>= 1.0', '< 2.0'
  spec.add_dependency 'rubocop-ast', '>= 1.0', '< 2.0'

  spec.add_development_dependency 'rake', '~> 13.0'
  spec.add_development_dependency 'rspec', '~> 3.12'
  spec.add_development_dependency 'simplecov', '~> 1.1'
end
