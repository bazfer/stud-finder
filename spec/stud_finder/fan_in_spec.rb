# frozen_string_literal: true

require 'spec_helper'
require 'stud_finder/fan_in'

RSpec.describe StudFinder::FanIn do
  def write_file(relative_path, content)
    path = File.join(repo_path, relative_path)
    FileUtils.mkdir_p(File.dirname(path))
    File.write(path, content)
  end

  def fan_in(files)
    described_class.new(repo_path: repo_path, files: files).call.counts
  end

  def result(files)
    described_class.new(repo_path: repo_path, files: files).call
  end

  let(:repo_path) { Dir.mktmpdir('stud-finder-fan-in') }

  after do
    FileUtils.remove_entry(repo_path)
  end

  it 'counts how many other files reference the primary constant' do
    write_file('app/models/user.rb', 'class User; end')
    write_file('app/services/greet_user.rb', 'class GreetUser; User.new; end')

    counts = fan_in(['app/models/user.rb', 'app/services/greet_user.rb'])

    expect(counts['app/models/user.rb']).to eq(1)
    expect(counts['app/services/greet_user.rb']).to eq(0)
  end

  it 'uses the Zeitwerk path as the primary constant for namespace-wrapped classes' do
    write_file('app/services/covalent_api/update_objective_templates.rb', <<~RUBY)
      module CovalentApi
        class UpdateObjectiveTemplates
        end
      end
    RUBY
    write_file('app/services/references_namespace.rb', 'class ReferencesNamespace; CovalentApi; end')
    write_file('app/services/references_service.rb', <<~RUBY)
      class ReferencesService
        CovalentApi::UpdateObjectiveTemplates.new
      end
    RUBY

    counts = fan_in([
                      'app/services/covalent_api/update_objective_templates.rb',
                      'app/services/references_namespace.rb',
                      'app/services/references_service.rb'
                    ])

    expect(counts['app/services/covalent_api/update_objective_templates.rb']).to eq(1)
  end

  it 'keeps separate ownership for multiple files in the same namespace' do
    write_file('app/services/covalent_api/update_objective_templates.rb', <<~RUBY)
      module CovalentApi
        class UpdateObjectiveTemplates
        end
      end
    RUBY
    write_file('app/services/covalent_api/sync_objectives.rb', <<~RUBY)
      module CovalentApi
        class SyncObjectives
        end
      end
    RUBY
    write_file('app/services/references_update.rb', <<~RUBY)
      class ReferencesUpdate
        CovalentApi::UpdateObjectiveTemplates.new
      end
    RUBY
    write_file('app/services/references_sync.rb', <<~RUBY)
      class ReferencesSync
        CovalentApi::SyncObjectives.new
      end
    RUBY
    write_file('app/services/references_namespace.rb', 'class ReferencesNamespace; CovalentApi; end')

    counts = fan_in([
                      'app/services/covalent_api/update_objective_templates.rb',
                      'app/services/covalent_api/sync_objectives.rb',
                      'app/services/references_update.rb',
                      'app/services/references_sync.rb',
                      'app/services/references_namespace.rb'
                    ])

    expect(counts['app/services/covalent_api/update_objective_templates.rb']).to eq(1)
    expect(counts['app/services/covalent_api/sync_objectives.rb']).to eq(1)
  end

  it 'uses the Zeitwerk path for single-class files without a namespace wrapper' do
    write_file('app/models/user.rb', 'class User; end')
    write_file('app/services/references_user.rb', 'class ReferencesUser; User.new; end')

    counts = fan_in(['app/models/user.rb', 'app/services/references_user.rb'])

    expect(counts['app/models/user.rb']).to eq(1)
  end

  it 'resolves bare constants against the enclosing namespace before top-level constants' do
    write_file('app/models/billing/invoice.rb', <<~RUBY)
      module Billing
        class Invoice; end
      end
    RUBY
    write_file('app/services/billing/processor.rb', <<~RUBY)
      module Billing
        class Processor
          Invoice.new
        end
      end
    RUBY

    r = result(['app/models/billing/invoice.rb', 'app/services/billing/processor.rb'])

    expect(r.counts['app/models/billing/invoice.rb']).to eq(1)
    expect(r.edges['app/models/billing/invoice.rb'][:dependents])
      .to contain_exactly('app/services/billing/processor.rb')
    expect(r.edges['app/services/billing/processor.rb'][:dependencies])
      .to contain_exactly('app/models/billing/invoice.rb')
  end

  it 'does not resolve bare constants to unrelated namespaces' do
    write_file('app/models/billing/invoice.rb', <<~RUBY)
      module Billing
        class Invoice; end
      end
    RUBY
    write_file('app/services/shipping/processor.rb', <<~RUBY)
      module Shipping
        class Processor
          Invoice.new
        end
      end
    RUBY

    r = result(['app/models/billing/invoice.rb', 'app/services/shipping/processor.rb'])

    expect(r.counts['app/models/billing/invoice.rb']).to eq(0)
    expect(r.edges['app/models/billing/invoice.rb'][:dependents]).to be_empty
    expect(r.edges['app/services/shipping/processor.rb'][:dependencies]).to be_empty
  end

  it 'resolves absolute constants from the top level only' do
    write_file('app/models/invoice.rb', 'class Invoice; end')
    write_file('app/models/billing/invoice.rb', <<~RUBY)
      module Billing
        class Invoice; end
      end
    RUBY
    write_file('app/services/billing/processor.rb', <<~RUBY)
      module Billing
        class Processor
          ::Invoice.new
        end
      end
    RUBY

    r = result(['app/models/invoice.rb', 'app/models/billing/invoice.rb', 'app/services/billing/processor.rb'])

    expect(r.counts['app/models/invoice.rb']).to eq(1)
    expect(r.counts['app/models/billing/invoice.rb']).to eq(0)
    expect(r.edges['app/models/invoice.rb'][:dependents]).to contain_exactly('app/services/billing/processor.rb')
    expect(r.edges['app/services/billing/processor.rb'][:dependencies]).to contain_exactly('app/models/invoice.rb')
  end

  it 'resolves deep bare constants from nearest enclosing namespace outward' do
    write_file('app/models/x.rb', 'class X; end')
    write_file('app/models/a/x.rb', 'module A; class X; end; end')
    write_file('app/models/a/b/x.rb', 'module A; module B; class X; end; end; end')
    write_file('app/models/a/b/c/x.rb', 'module A; module B; module C; class X; end; end; end; end')
    write_file('app/services/a/b/c/processor.rb', <<~RUBY)
      module A
        module B
          module C
            class Processor
              X.new
            end
          end
        end
      end
    RUBY

    r = result([
                 'app/models/x.rb',
                 'app/models/a/x.rb',
                 'app/models/a/b/x.rb',
                 'app/models/a/b/c/x.rb',
                 'app/services/a/b/c/processor.rb'
               ])

    expect(r.counts['app/models/a/b/c/x.rb']).to eq(1)
    expect(r.counts['app/models/a/b/x.rb']).to eq(0)
    expect(r.counts['app/models/a/x.rb']).to eq(0)
    expect(r.counts['app/models/x.rb']).to eq(0)
    expect(r.edges['app/services/a/b/c/processor.rb'][:dependencies]).to contain_exactly('app/models/a/b/c/x.rb')
  end

  it 'does not fall through compact qualified class namespace segments' do
    write_file('app/models/x.rb', 'class X; end')
    write_file('app/models/a/b/c/x.rb', 'module A; module B; module C; class X; end; end; end; end')
    write_file('app/services/a/b/c/processor.rb', <<~RUBY)
      class A::B::C::Processor
        X.new
      end
    RUBY

    r = result(['app/models/x.rb', 'app/models/a/b/c/x.rb', 'app/services/a/b/c/processor.rb'])

    expect(r.counts['app/models/x.rb']).to eq(1)
    expect(r.counts['app/models/a/b/c/x.rb']).to eq(0)
    expect(r.edges['app/services/a/b/c/processor.rb'][:dependencies]).to contain_exactly('app/models/x.rb')
  end

  it 'falls back through deep bare constant candidates in lexical order' do
    write_file('app/models/x.rb', 'class X; end')
    write_file('app/models/a/x.rb', 'module A; class X; end; end')
    write_file('app/services/a/b/c/processor.rb', <<~RUBY)
      module A
        module B
          module C
            class Processor
              X.new
            end
          end
        end
      end
    RUBY

    r = result(['app/models/x.rb', 'app/models/a/x.rb', 'app/services/a/b/c/processor.rb'])

    expect(r.counts['app/models/a/x.rb']).to eq(1)
    expect(r.counts['app/models/x.rb']).to eq(0)
    expect(r.edges['app/services/a/b/c/processor.rb'][:dependencies]).to contain_exactly('app/models/a/x.rb')
  end

  it 'resolves partially qualified constants against enclosing namespaces first' do
    write_file('config/concerns_authenticatable.rb', 'class Concerns::Authenticatable; end')
    write_file('config/admin_concerns_authenticatable.rb', 'class Admin::Concerns::Authenticatable; end')
    write_file('app/controllers/admin/controller.rb', <<~RUBY)
      module Admin
        class Controller
          include Concerns::Authenticatable
        end
      end
    RUBY

    r = result([
                 'config/concerns_authenticatable.rb',
                 'config/admin_concerns_authenticatable.rb',
                 'app/controllers/admin/controller.rb'
               ])

    expect(r.counts['config/admin_concerns_authenticatable.rb']).to eq(1)
    expect(r.counts['config/concerns_authenticatable.rb']).to eq(0)
    expect(r.edges['app/controllers/admin/controller.rb'][:dependencies])
      .to contain_exactly('config/admin_concerns_authenticatable.rb')
  end

  it 'falls back partially qualified constants to the top level' do
    write_file('config/concerns_authenticatable.rb', 'class Concerns::Authenticatable; end')
    write_file('app/controllers/admin/controller.rb', <<~RUBY)
      module Admin
        class Controller
          include Concerns::Authenticatable
        end
      end
    RUBY

    r = result(['config/concerns_authenticatable.rb', 'app/controllers/admin/controller.rb'])

    expect(r.counts['config/concerns_authenticatable.rb']).to eq(1)
    expect(r.edges['app/controllers/admin/controller.rb'][:dependencies])
      .to contain_exactly('config/concerns_authenticatable.rb')
  end

  it 'resolves superclass constants against the enclosing namespace before top-level constants' do
    write_file('app/models/base.rb', 'class Base; end')
    write_file('app/models/a/base.rb', 'module A; class Base; end; end')
    write_file('app/models/a/foo.rb', <<~RUBY)
      module A
        class Foo < Base
        end
      end
    RUBY

    r = result(['app/models/base.rb', 'app/models/a/base.rb', 'app/models/a/foo.rb'])

    expect(r.counts['app/models/a/base.rb']).to eq(1)
    expect(r.counts['app/models/base.rb']).to eq(0)
    expect(r.edges['app/models/a/foo.rb'][:dependencies]).to contain_exactly('app/models/a/base.rb')
  end

  it 'resolves included constants against the enclosing namespace before top-level constants' do
    write_file('app/models/base.rb', 'class Base; end')
    write_file('app/models/a/base.rb', 'module A; class Base; end; end')
    write_file('app/models/a/foo.rb', <<~RUBY)
      module A
        class Foo
          include Base
        end
      end
    RUBY

    r = result(['app/models/base.rb', 'app/models/a/base.rb', 'app/models/a/foo.rb'])

    expect(r.counts['app/models/a/base.rb']).to eq(1)
    expect(r.counts['app/models/base.rb']).to eq(0)
    expect(r.edges['app/models/a/foo.rb'][:dependencies]).to contain_exactly('app/models/a/base.rb')
  end

  it 'warns and skips a reference when candidate resolution fails' do
    write_file('app/models/user.rb', 'class User; end')
    stderr = StringIO.new
    fan_in = described_class.new(repo_path: repo_path, files: ['app/models/user.rb'], stderr: stderr)

    allow(fan_in).to receive(:lexical_namespace).and_raise(StandardError, 'boom')

    expect { fan_in.call }.not_to raise_error
    expect(stderr.string).to include('Warning: fan_in_reference_resolution_failed: StandardError: boom')
  end

  it 'maps concerns to the constant below the concerns directory' do
    write_file('app/models/concerns/auditable.rb', 'AUDITABLE = true')
    write_file('app/models/user.rb', 'class User; include Auditable; end')

    counts = fan_in(['app/models/concerns/auditable.rb', 'app/models/user.rb'])

    expect(counts['app/models/concerns/auditable.rb']).to eq(1)
  end

  it 'falls back to the first top-level class or module outside Zeitwerk paths' do
    write_file('config/initializers/multi.rb', <<~RUBY)
      class FirstConstant; end
      class SecondConstant; end
    RUBY
    write_file('app/services/uses_first.rb', 'class UsesFirst; FirstConstant.new; end')
    write_file('app/services/uses_second.rb', 'class UsesSecond; SecondConstant.new; end')

    counts = fan_in(['config/initializers/multi.rb', 'app/services/uses_first.rb', 'app/services/uses_second.rb'])

    expect(counts['config/initializers/multi.rb']).to eq(1)
  end

  it 'does not treat nested classes as primary constants' do
    write_file('app/models/foo.rb', <<~RUBY)
      class Foo
        class Bar; end
      end
    RUBY
    write_file('app/services/uses_foo.rb', 'class UsesFoo; Foo.new; end')
    write_file('app/services/uses_bar.rb', 'class UsesBar; Foo::Bar.new; end')

    counts = fan_in(['app/models/foo.rb', 'app/services/uses_foo.rb', 'app/services/uses_bar.rb'])

    expect(counts['app/models/foo.rb']).to eq(1)
  end

  it 'counts references from test files toward app and lib constants' do
    write_file('app/models/user.rb', 'class User; end')
    write_file('lib/api_client.rb', 'class ApiClient; end')
    write_file('test/models/user_test.rb', <<~RUBY)
      class UserTest
        User.new
        ApiClient.new
      end
    RUBY

    counts = fan_in(['app/models/user.rb', 'lib/api_client.rb', 'test/models/user_test.rb'])

    expect(counts['app/models/user.rb']).to eq(1)
    expect(counts['lib/api_client.rb']).to eq(1)
  end

  it 'assigns zero fan_in to test files because they do not own constants' do
    write_file('test/models/user_test.rb', 'class UserTest; UserTest.new; end')
    write_file('app/models/user.rb', 'class User; end')

    counts = fan_in(['test/models/user_test.rb', 'app/models/user.rb'])

    expect(counts['test/models/user_test.rb']).to eq(0)
  end

  it 'does not count a file reference to its own constant' do
    write_file('app/models/user.rb', 'class User; User.new; end')

    counts = fan_in(['app/models/user.rb'])

    expect(counts['app/models/user.rb']).to eq(0)
  end

  it 'silently ignores unknown or unresolvable constants' do
    write_file('app/models/user.rb', 'class User; end')
    write_file('app/services/uses_unknown.rb', 'class UsesUnknown; MissingConstant.new; end')

    expect { fan_in(['app/models/user.rb', 'app/services/uses_unknown.rb']) }.not_to raise_error
  end

  it 'assigns zero fan_in to files outside app, lib, or test with no top-level constant' do
    write_file('config/routes.rb', 'Rails.application.routes.draw do; end')
    write_file('app/models/user.rb', 'class User; end')

    counts = fan_in(['config/routes.rb', 'app/models/user.rb'])

    expect(counts['config/routes.rb']).to eq(0)
  end

  it 'computes fan_out as the number of known files this file depends on' do
    write_file('app/models/user.rb', 'class User; end')
    write_file('app/models/role.rb', 'class Role; end')
    write_file('app/services/greet_user.rb', 'class GreetUser; User.new; Role.new; end')

    r = result(['app/models/user.rb', 'app/models/role.rb', 'app/services/greet_user.rb'])

    expect(r.fan_out_counts['app/services/greet_user.rb']).to eq(2)
    expect(r.fan_out_counts['app/models/user.rb']).to eq(0)
    expect(r.fan_out_counts['app/models/role.rb']).to eq(0)
  end

  it 'populates dependents and dependencies in edges' do
    write_file('app/models/user.rb', 'class User; end')
    write_file('app/services/greet_user.rb', 'class GreetUser; User.new; end')

    r = result(['app/models/user.rb', 'app/services/greet_user.rb'])

    expect(r.edges['app/models/user.rb'][:dependents]).to contain_exactly('app/services/greet_user.rb')
    expect(r.edges['app/models/user.rb'][:dependencies]).to be_empty
    expect(r.edges['app/services/greet_user.rb'][:dependents]).to be_empty
    expect(r.edges['app/services/greet_user.rb'][:dependencies]).to contain_exactly('app/models/user.rb')
  end

  it 'computes instability as 0.0 for an isolated file' do
    write_file('app/models/standalone.rb', 'class Standalone; end')

    r = result(['app/models/standalone.rb'])

    expect(r.fan_out_counts['app/models/standalone.rb']).to eq(0)
    expect(r.edges['app/models/standalone.rb'][:dependents]).to be_empty
    expect(r.edges['app/models/standalone.rb'][:dependencies]).to be_empty
  end
end
