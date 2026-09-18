# frozen_string_literal: true

require 'spec_helper'
require 'stud_finder/js_complexity'

RSpec.describe StudFinder::JsComplexity do
  def make_repo
    Dir.mktmpdir do |dir|
      files = %w[src/a.js src/b.js src/c.ts]
      files.each { |file| write_file(dir, file, '') }
      yield dir, files
    end
  end

  def make_js_repo(files = %w[src/a.js src/b.js])
    Dir.mktmpdir do |dir|
      files.each { |file| write_file(dir, file, '') }
      yield dir, files
    end
  end

  def write_file(root, relative, content)
    path = File.join(root, relative)
    FileUtils.mkdir_p(File.dirname(path))
    File.write(path, content)
  end

  def write_eslint(root, body)
    path = File.join(root, 'node_modules/.bin/eslint')
    FileUtils.mkdir_p(File.dirname(path))
    File.write(path, body)
    FileUtils.chmod('+x', path)
    path
  end

  def call(root, files, js_timeout: 60, stderr: StringIO.new)
    described_class.new(repo_path: root, files: files, js_timeout: js_timeout, stderr: stderr).call
  end

  it 'parses ESLint JSON and keeps the max complexity per file' do
    make_repo do |root, files|
      write_eslint(root, <<~SH)
        #!/bin/sh
        if [ "$1" = "--version" ]; then echo v9.1.0; exit 0; fi
        cat <<JSON
        [{"filePath":"#{root}/src/a.js","messages":[
          {"message":"Function has a complexity of 2. Maximum allowed is 0."},
          {"message":"Function has a complexity of 7. Maximum allowed is 0."}]},
         {"filePath":"#{root}/src/b.js","messages":[
          {"message":"Function has a complexity of 3. Maximum allowed is 0."}]}]
        JSON
      SH

      result = call(root, files)

      expect(result.counts).to include('src/a.js' => 7, 'src/b.js' => 3, 'src/c.ts' => 0)
    end
  end

  it 'parses ESLint JSON from expected violation exit status without warning' do
    make_js_repo(%w[src/a.js]) do |root, files|
      write_eslint(root, <<~SH)
        #!/bin/sh
        if [ "$1" = "--version" ]; then echo v9.1.0; exit 0; fi
        cat <<JSON
        [{"filePath":"#{root}/src/a.js","messages":[
          {"message":"Function has a complexity of 5. Maximum allowed is 0."}]}]
        JSON
        exit 1
      SH
      stderr = StringIO.new

      result = call(root, files, stderr: stderr)

      expect(result.counts).to eq('src/a.js' => 5)
      expect(result.warnings).to be_empty
      expect(stderr.string).not_to include('js_eslint_failed')
    end
  end

  it 'maps physical ESLint paths back to a repository opened through a symlink' do
    make_js_repo(%w[src/a.js]) do |root, files|
      alias_path = File.join(root, 'alias')
      File.symlink(root, alias_path)
      write_eslint(root, <<~SH)
        #!/bin/sh
        if [ "$1" = "--version" ]; then echo v9.1.0; exit 0; fi
        echo '[{"filePath":"#{File.realpath(root)}/src/a.js","messages":[
          {"message":"Function has a complexity of 5. Maximum allowed is 0."}]}]'
      SH

      expect(call(alias_path, files).counts).to eq('src/a.js' => 5)
    end
  end

  it 'uses v9 flat config and deletes the temporary config' do
    make_repo do |root, files|
      args_file = File.join(root, 'args.txt')
      write_eslint(root, <<~SH)
        #!/bin/sh
        if [ "$1" = "--version" ]; then echo v9.2.0; exit 0; fi
        printf '%s\n' "$@" > #{args_file}
        echo '[]'
      SH

      result = call(root, files)
      args = File.read(args_file).lines.map(&:chomp)
      config_path = args[args.index('--config') + 1]

      expect(result.warnings).to include('js_ts_parser_missing')
      expect(args).to include('--config')
      expect(args).not_to include('--no-eslintrc', '--resolve-plugins-relative-to', '--parser', '--parser-options')
      expect(File.exist?(config_path)).to be(false)
    end
  end

  it 'uses v8 CLI flags without a temp config' do
    make_repo do |root, files|
      args_file = File.join(root, 'args.txt')
      write_eslint(root, <<~SH)
        #!/bin/sh
        if [ "$1" = "--version" ]; then echo v8.57.0; exit 0; fi
        printf '%s\n' "$@" > #{args_file}
        echo '[]'
      SH

      call(root, files)
      args = File.read(args_file).lines.map(&:chomp)

      expect(args).to include('--no-eslintrc', '--resolve-plugins-relative-to', '.',
                              '--parser-options=ecmaVersion:2022,sourceType:module')
      expect(args).not_to include('--config')
    end
  end

  it 'degrades TypeScript files to zero when the TS parser is missing' do
    make_repo do |root, files|
      write_eslint(root, <<~SH)
        #!/bin/sh
        if [ "$1" = "--version" ]; then echo v9.1.0; exit 0; fi
        for arg in "$@"; do [ "$arg" = "src/c.ts" ] && exit 9; done
        echo '[{"filePath":"#{root}/src/a.js","messages":[{"message":"Function has a complexity of 4. Maximum allowed is 0."}]}]'
      SH
      stderr = StringIO.new

      result = call(root, files, stderr: stderr)

      expect(result.counts['src/a.js']).to eq(4)
      expect(result.counts['src/c.ts']).to eq(0)
      expect(result.warnings).to include('js_ts_parser_missing')
      expect(stderr.string).to include('js_ts_parser_missing')
    end
  end

  it 'runs files in batches of 500' do
    Dir.mktmpdir do |root|
      files = 501.times.map { |i| "src/file#{i}.js" }
      files.each { |file| write_file(root, file, '') }
      counter = File.join(root, 'count.txt')
      write_eslint(root, <<~SH)
        #!/bin/sh
        if [ "$1" = "--version" ]; then echo v9.1.0; exit 0; fi
        count=0; [ -f #{counter} ] && count=$(cat #{counter}); count=$((count + 1)); echo $count > #{counter}
        echo '[]'
      SH

      call(root, files)

      expect(File.read(counter).to_i).to eq(2)
    end
  end

  it 'zeros a timed-out batch and continues' do
    make_repo do |root, files|
      write_eslint(root, "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo v9.1.0; exit 0; fi\necho '[]'\n")
      allow(Timeout).to receive(:timeout).and_call_original
      allow(Timeout).to receive(:timeout).with(1).and_raise(Timeout::Error)

      result = call(root, files, js_timeout: 1)

      expect(result.counts.values).to all(eq(0))
      expect(result.warnings).to include('js_eslint_timeout')
    end
  end

  it 'warns and zeros the affected batch when ESLint exits non-zero with stderr only' do
    make_js_repo do |root, files|
      write_eslint(root, <<~SH)
        #!/bin/sh
        if [ "$1" = "--version" ]; then echo v9.1.0; exit 0; fi
        echo 'fatal eslint failure' >&2
        exit 2
      SH
      stderr = StringIO.new

      result = call(root, files, stderr: stderr)

      expect(result.counts).to eq('src/a.js' => 0, 'src/b.js' => 0)
      expect(result.warnings).to eq(['js_eslint_failed'])
      expect(stderr.string).to include('js_eslint_failed')
    end
  end

  it 'warns and zeros malformed ESLint JSON output while continuing later batches' do
    Dir.mktmpdir do |root|
      files = 501.times.map { |i| "src/file#{i}.js" }
      files.each { |file| write_file(root, file, '') }
      counter = File.join(root, 'count.txt')
      write_eslint(root, <<~SH)
        #!/bin/sh
        if [ "$1" = "--version" ]; then echo v9.1.0; exit 0; fi
        count=0; [ -f #{counter} ] && count=$(cat #{counter}); count=$((count + 1)); echo $count > #{counter}
        if [ "$count" = "1" ]; then echo '[not json'; exit 0; fi
        echo '[{"filePath":"#{root}/src/file500.js","messages":[{"message":"Function has a complexity of 6. Maximum allowed is 0."}]}]'
      SH
      stderr = StringIO.new

      result = call(root, files, stderr: stderr)

      expect(result.counts['src/file0.js']).to eq(0)
      expect(result.counts['src/file500.js']).to eq(6)
      expect(result.warnings).to eq(['js_eslint_malformed'])
      expect(stderr.string).to include('js_eslint_malformed')
    end
  end

  it 'degrades when ESLint is missing' do
    make_repo do |root, files|
      original_path = ENV.fetch('PATH', nil)
      ENV['PATH'] = ''
      stderr = StringIO.new

      result = call(root, files, stderr: stderr)

      expect(result.counts.values).to all(eq(0))
      expect(result.warnings).to eq(['js_eslint_missing'])
      expect(stderr.string).to include('js_eslint_missing')
    ensure
      ENV['PATH'] = original_path
    end
  end
end
