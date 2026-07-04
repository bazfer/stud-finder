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

  def result_without_rails_inference(files)
    described_class.new(repo_path: repo_path, files: files, rails_inference: false).call
  end

  def fixture_files
    Dir.chdir(repo_path) { Dir['{app,lib,spec}/**/*.rb'].sort }
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

  it 'does not fall back partially qualified constants after a namespace root match' do
    write_file('config/concerns_authenticatable.rb', 'class Concerns::Authenticatable; end')
    write_file('app/models/admin/concerns.rb', <<~RUBY)
      module Admin
        module Concerns
        end
      end
    RUBY
    write_file('app/controllers/admin/controller.rb', <<~RUBY)
      module Admin
        class Controller
          include Concerns::Authenticatable
        end
      end
    RUBY

    r = result([
                 'config/concerns_authenticatable.rb',
                 'app/models/admin/concerns.rb',
                 'app/controllers/admin/controller.rb'
               ])

    expect(r.counts['config/concerns_authenticatable.rb']).to eq(0)
    expect(r.counts['app/models/admin/concerns.rb']).to eq(0)
    expect(r.edges['app/controllers/admin/controller.rb'][:dependencies]).to be_empty
  end

  it 'falls back partially qualified constants to the top level when the namespace root is missing' do
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

  it 'resolves declaration identifiers against the surrounding scope, not the class or module being declared' do
    write_file('app/models/billing.rb', 'class Billing; end')
    write_file('app/models/billing/billing.rb', <<~RUBY)
      module Billing
        class Billing; end
      end
    RUBY
    write_file('app/services/billing_facade.rb', 'module Billing; end')

    r = result(['app/models/billing.rb', 'app/models/billing/billing.rb', 'app/services/billing_facade.rb'])

    expect(r.counts['app/models/billing.rb']).to eq(2)
    expect(r.counts['app/models/billing/billing.rb']).to eq(0)
    expect(r.edges['app/services/billing_facade.rb'][:dependencies]).to contain_exactly('app/models/billing.rb')
  end

  it 'resolves compact declaration identifiers against the surrounding scope' do
    write_file('app/models/a/b.rb', 'module A::B; end')
    write_file('app/models/a/b/b.rb', <<~RUBY)
      module A
        module B
          module B; end
        end
      end
    RUBY
    write_file('app/services/opens_a_b.rb', 'module A::B; end')

    r = result(['app/models/a/b.rb', 'app/models/a/b/b.rb', 'app/services/opens_a_b.rb'])

    expect(r.counts['app/models/a/b.rb']).to eq(2)
    expect(r.counts['app/models/a/b/b.rb']).to eq(0)
    expect(r.edges['app/services/opens_a_b.rb'][:dependencies]).to contain_exactly('app/models/a/b.rb')
  end

  it 'resolves superclass constants against the surrounding scope, not the class being declared' do
    write_file('app/models/a/base.rb', 'module A; class Base; end; end')
    write_file('app/models/a/foo/base.rb', 'module A; class Foo; class Base; end; end; end')
    write_file('app/models/a/foo.rb', <<~RUBY)
      module A
        class Foo < Base
        end
      end
    RUBY

    r = result(['app/models/a/base.rb', 'app/models/a/foo/base.rb', 'app/models/a/foo.rb'])

    expect(r.counts['app/models/a/base.rb']).to eq(1)
    expect(r.counts['app/models/a/foo/base.rb']).to eq(0)
    expect(r.edges['app/models/a/foo.rb'][:dependencies]).to contain_exactly('app/models/a/base.rb')
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

  describe 'Rails inference' do
    let(:source_fixture) { File.expand_path('../fixtures/sample_app', __dir__) }

    before do
      FileUtils.cp_r(Dir.glob(File.join(source_fixture, '*')), repo_path)
    end

    it 'infers direct string literal constantize references' do
      write_file('app/models/user.rb', 'class User; end')
      write_file('app/services/loader.rb', <<~RUBY)
        class Loader
          "User".constantize
        end
      RUBY

      r = result(['app/models/user.rb', 'app/services/loader.rb'])

      expect(r.counts['app/models/user.rb']).to eq(1)
      expect(r.edges['app/models/user.rb'][:dependents]).to include('app/services/loader.rb')
    end

    it 'resolves direct string literal constantize references from the top level' do
      write_file('app/models/user.rb', 'class User; end')
      write_file('app/models/tenant/user.rb', 'module Tenant; class User; end; end')
      write_file('app/services/tenant/loader.rb', <<~RUBY)
        module Tenant
          class Loader
            "User".constantize
          end
        end
      RUBY

      r = result(['app/models/user.rb', 'app/models/tenant/user.rb', 'app/services/tenant/loader.rb'])

      expect(r.counts['app/models/user.rb']).to eq(1)
      expect(r.counts['app/models/tenant/user.rb']).to eq(0)
      expect(r.edges['app/services/tenant/loader.rb'][:dependencies]).to contain_exactly('app/models/user.rb')
    end

    it 'does not infer transformed string literal constantize chains' do
      write_file('app/models/user.rb', 'class User; end')
      write_file('app/services/loader.rb', <<~RUBY)
        class Loader
          "User".downcase.constantize
        end
      RUBY

      r = result(['app/models/user.rb', 'app/services/loader.rb'])

      expect(r.counts['app/models/user.rb']).to eq(0)
      expect(r.edges['app/models/user.rb'][:dependents]).not_to include('app/services/loader.rb')
    end

    it 'counts belongs_to association references in the fixture app' do
      File.write(File.join(repo_path, 'app/models/post.rb'), <<~RUBY)
        # frozen_string_literal: true

        class Post
          belongs_to :user
        end
      RUBY

      r = result(fixture_files)

      expect(r.counts['app/models/user.rb']).to eq(9)
      expect(r.edges['app/models/user.rb'][:dependents]).to include('app/models/post.rb')
    end

    it 'infers has_many association references with heuristic singularization' do
      File.write(File.join(repo_path, 'app/models/post.rb'), <<~RUBY)
        # frozen_string_literal: true

        class Post
          has_many :comments
        end
      RUBY

      r = result(fixture_files)

      expect(r.counts['app/models/comment.rb']).to eq(1)
      expect(r.edges['app/models/comment.rb'][:dependents]).to include('app/models/post.rb')
    end

    it 'keeps inferring parent-side polymorphic has_many associations' do
      write_file('app/models/comment.rb', 'class Comment; end')
      write_file('app/models/post.rb', <<~RUBY)
        class Post
          has_many :comments, as: :commentable
        end
      RUBY

      r = result(['app/models/comment.rb', 'app/models/post.rb'])

      expect(r.counts['app/models/comment.rb']).to eq(1)
      expect(r.edges['app/models/comment.rb'][:dependents]).to include('app/models/post.rb')
    end

    it 'does not infer literal polymorphic belongs_to associations' do
      write_file('app/models/commentable.rb', 'module Commentable; end')
      write_file('app/models/comment.rb', <<~RUBY)
        class Comment
          belongs_to :commentable, polymorphic: true
        end
      RUBY

      r = result(['app/models/commentable.rb', 'app/models/comment.rb'])

      expect(r.counts['app/models/commentable.rb']).to eq(0)
      expect(r.edges['app/models/commentable.rb'][:dependents]).not_to include('app/models/comment.rb')
      expect(r.edges['app/models/comment.rb'][:dependencies]).not_to include('app/models/commentable.rb')
    end

    it 'does not infer dynamic polymorphic belongs_to associations' do
      write_file('app/models/commentable.rb', 'module Commentable; end')
      write_file('app/models/comment.rb', <<~RUBY)
        class Comment
          polymorphic_association = true
          belongs_to :commentable, polymorphic: polymorphic_association
        end
      RUBY

      r = result(['app/models/commentable.rb', 'app/models/comment.rb'])

      expect(r.counts['app/models/commentable.rb']).to eq(0)
      expect(r.edges['app/models/commentable.rb'][:dependents]).not_to include('app/models/comment.rb')
      expect(r.edges['app/models/comment.rb'][:dependencies]).not_to include('app/models/commentable.rb')
    end

    it 'lets string class_name override the association symbol' do
      File.write(File.join(repo_path, 'app/models/post.rb'), <<~RUBY)
        # frozen_string_literal: true

        class Post
          belongs_to :author, class_name: 'Profile'
        end
      RUBY

      r = result(fixture_files)

      expect(r.counts['app/models/profile.rb']).to eq(2)
      expect(r.edges['app/models/profile.rb'][:dependents]).to include('app/models/post.rb')
      expect(r.edges['app/models/user.rb'][:dependents]).not_to include('app/models/post.rb')
    end

    it 'ignores dynamic class_name values without falling back to the symbol' do
      write_file('app/models/thing.rb', 'class Thing; end')
      File.write(File.join(repo_path, 'app/models/post.rb'), <<~RUBY)
        # frozen_string_literal: true

        class Post
          suffix = 'Thing'
          belongs_to :thing, class_name: "Dynamic\#{suffix}"
        end
      RUBY

      r = result(fixture_files)

      expect(r.counts['app/models/thing.rb']).to eq(0)
      expect(r.warnings).not_to include('fan_in_rails_inference_failed')
    end

    it 'resolves namespaced associations against the enclosing scope before top-level constants' do
      write_file('app/models/item.rb', 'class Item; end')
      write_file('app/models/store/item.rb', 'module Store; class Item; end; end')
      write_file('app/models/store/order.rb', <<~RUBY)
        module Store
          class Order
            belongs_to :item
          end
        end
      RUBY

      r = result(['app/models/item.rb', 'app/models/store/item.rb', 'app/models/store/order.rb'])

      expect(r.counts['app/models/store/item.rb']).to eq(1)
      expect(r.counts['app/models/item.rb']).to eq(0)
      expect(r.edges['app/models/store/item.rb'][:dependents]).to contain_exactly('app/models/store/order.rb')
    end

    it 'infers namespaced associations inside with_options class-body wrappers' do
      write_file('app/models/comment.rb', 'class Comment; end')
      write_file('app/models/store/comment.rb', 'module Store; class Comment; end; end')
      write_file('app/models/store/post.rb', <<~RUBY)
        module Store
          class Post
            with_options dependent: :destroy do
              has_many :comments
            end
          end
        end
      RUBY

      r = result(['app/models/comment.rb', 'app/models/store/comment.rb', 'app/models/store/post.rb'])

      expect(r.counts['app/models/store/comment.rb']).to eq(1)
      expect(r.counts['app/models/comment.rb']).to eq(0)
      expect(r.edges['app/models/store/comment.rb'][:dependents]).to contain_exactly('app/models/store/post.rb')
    end

    it 'infers associations inside included concern wrappers' do
      write_file('app/models/account.rb', 'class Account; end')
      write_file('app/models/concerns/commentable.rb', <<~RUBY)
        module Commentable
          included do
            belongs_to :account
          end
        end
      RUBY
      write_file('app/models/user.rb', <<~RUBY)
        class User
          include Commentable
        end
      RUBY

      r = result(['app/models/account.rb', 'app/models/concerns/commentable.rb', 'app/models/user.rb'])

      expect(r.counts['app/models/account.rb']).to eq(1)
      expect(r.edges['app/models/account.rb'][:dependents]).to contain_exactly('app/models/concerns/commentable.rb')
    end

    it 'can disable Rails inference' do
      File.write(File.join(repo_path, 'app/models/post.rb'), <<~RUBY)
        # frozen_string_literal: true

        class Post
          belongs_to :user
        end
      RUBY

      r = result_without_rails_inference(fixture_files)

      expect(r.counts['app/models/user.rb']).to eq(8)
      expect(r.edges['app/models/user.rb'][:dependents]).not_to include('app/models/post.rb')
    end
  end

  it 'warns and skips a reference when candidate resolution fails' do
    write_file('app/models/user.rb', 'class User; end')
    stderr = StringIO.new
    fan_in = described_class.new(repo_path: repo_path, files: ['app/models/user.rb'], stderr: stderr)

    allow(fan_in).to receive(:lexical_namespace).and_raise(StandardError, 'boom')

    result = nil
    expect { result = fan_in.call }.not_to raise_error

    expect(result.warnings).to eq(['fan_in_reference_resolution_failed'])
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
