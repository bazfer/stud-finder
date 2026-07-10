# frozen_string_literal: true

require 'spec_helper'
require 'stud_finder/js_fan_in'

RSpec.describe StudFinder::JsFanIn do
  def make_repo
    Dir.mktmpdir do |dir|
      system('git', 'init', '-q', dir)
      yield dir
    end
  end

  def write_file(root, relative, content = '')
    path = File.join(root, relative)
    FileUtils.mkdir_p(File.dirname(path))
    File.write(path, content)
  end

  def write_depcruise(root, body)
    path = File.join(root, 'node_modules/.bin/depcruise')
    FileUtils.mkdir_p(File.dirname(path))
    File.write(path, body)
    FileUtils.chmod('+x', path)
  end

  def call(root, files, js_timeout: 60, stderr: StringIO.new)
    described_class.new(repo_path: root, files: files, js_timeout: js_timeout, stderr: stderr).call
  end

  def warning_codes(warnings)
    warnings.map { |warning| warning.is_a?(Hash) ? warning.fetch(:code) : warning }
  end

  it 'counts incoming edges, deduplicates source-target pairs, and filters self/external deps' do
    make_repo do |root|
      files = %w[src/a.js src/b.ts src/c.jsx src/d.tsx]
      files.each { |file| write_file(root, file) }
      write_depcruise(root, <<~SH)
        #!/bin/sh
        cat <<'JSON'
        {"modules":[
          {"source":"./src/a.js","dependencies":[
            {"resolved":"./src/b.ts"},
            {"resolved":"./src/b.ts"},
            {"resolved":"./src/a.js"},
            {"resolved":"./node_modules/react/index.js"}]},
          {"source":"./src/c.jsx","dependencies":[{"resolved":"./src/b.ts"},{"resolved":"./src/d.tsx"}]},
          {"source":"./src/d.tsx","dependencies":[{"resolved":"./src/b.ts"}]}
        ]}
        JSON
      SH

      result = call(root, files)

      expect(result.warnings).to eq([])
      expect(result.counts).to include('src/a.js' => 0, 'src/b.ts' => 3, 'src/c.jsx' => 0, 'src/d.tsx' => 1)
    end
  end

  it 'probes dependency-cruiser in the target project node_modules' do
    make_repo do |root|
      files = %w[src/a.js src/b.js]
      status = instance_double(Process::Status, success?: true)
      allow(Open3).to receive(:capture3).with('node', '--version').and_return(['v24.0.0', '', status])
      allow(File).to receive(:executable?).and_call_original
      expect(File).to receive(:executable?).with(File.join(root, 'node_modules/.bin/depcruise')).and_return(false)
      original_path = ENV.fetch('PATH', nil)
      ENV['PATH'] = ''

      result = call(root, files)

      expect(result.warnings).to eq(['js_tools_missing'])
    ensure
      ENV['PATH'] = original_path
    end
  end

  it 'runs dependency-cruiser from the target project directory' do
    make_repo do |root|
      files = %w[src/a.js src/b.js]
      files.each { |file| write_file(root, file) }
      write_depcruise(root, "#!/bin/sh\nexit 0\n")
      status = instance_double(Process::Status, success?: true)
      depcruise = File.join(root, 'node_modules/.bin/depcruise')
      allow(Open3).to receive(:capture3).with('node', '--version').and_return(['v24.0.0', '', status])
      expect(Open3).to receive(:capture3)
        .with(depcruise, '--output-type', 'json', '.', chdir: root)
        .and_return(['{"modules":[]}', '', status])

      result = call(root, files)

      expect(result.warnings).to eq([])
      expect(result.counts).to eq('src/a.js' => 0, 'src/b.js' => 0)
    end
  end

  it 'degrades when node is missing' do
    make_repo do |root|
      files = %w[src/a.js src/b.js]
      allow(Open3).to receive(:capture3).with('node', '--version').and_raise(Errno::ENOENT)
      stderr = StringIO.new

      result = call(root, files, stderr: stderr)

      expect(result.counts.values).to all(eq(0))
      expect(result.warnings).to eq(['js_tools_missing'])
      expect(stderr.string).to include('js_tools_missing')
    end
  end

  it 'degrades when dependency-cruiser is missing, including a broken local bin path' do
    make_repo do |root|
      files = %w[src/a.js src/b.js]
      status = instance_double(Process::Status, success?: true)
      allow(Open3).to receive(:capture3).with('node', '--version').and_return(['v24.0.0', '', status])
      FileUtils.mkdir_p(File.join(root, 'node_modules/.bin'))
      FileUtils.ln_s(File.join(root, 'missing-depcruise'), File.join(root, 'node_modules/.bin/depcruise'))
      original_path = ENV.fetch('PATH', nil)
      ENV['PATH'] = ''

      result = call(root, files)

      expect(result.counts.values).to all(eq(0))
      expect(result.warnings).to eq(['js_tools_missing'])
    ensure
      ENV['PATH'] = original_path
    end
  end

  it 'retries dependency-cruiser with --no-config and warns that aliases may undercount' do
    make_repo do |root|
      files = %w[src/a.js src/b.js]
      files.each { |file| write_file(root, file) }
      calls = File.join(root, 'depcruise-calls')
      write_depcruise(root, <<~SH)
        #!/bin/sh
        echo "$*" >> depcruise-calls
        case " $* " in
          *" --no-config "*)
            cat <<'JSON'
        {"modules":[
          {"source":"./src/a.js","dependencies":[{"resolved":"./src/b.js"}]},
          {"source":"./src/b.js","dependencies":[]}
        ]}
        JSON
            ;;
          *)
            echo "No dependency-cruiser config found" >&2
            exit 1
            ;;
        esac
      SH
      stderr = StringIO.new

      result = call(root, files, stderr: stderr)

      expect(File.readlines(calls).map(&:strip)).to eq(
        [
          '--output-type json .',
          '--output-type json . --no-config'
        ]
      )
      expect(warning_codes(result.warnings)).to eq(['js_depcruise_no_config'])
      expect(result.warnings.first.fetch(:message)).to include('Path aliases')
      expect(result.counts).to eq('src/a.js' => 0, 'src/b.js' => 1)
      expect(result.fan_out_counts).to eq('src/a.js' => 1, 'src/b.js' => 0)
      expect(stderr.string).to include('Warning: js_depcruise_no_config: no dependency-cruiser config found')
    end
  end

  it 'warns with the primary dependency-cruiser stderr when both attempts fail' do
    make_repo do |root|
      files = %w[src/a.js src/b.js]
      files.each { |file| write_file(root, file) }
      write_depcruise(root, <<~SH)
        #!/bin/sh
        case " $* " in
          *" --no-config "*)
            echo "retry also failed" >&2
            exit 2
            ;;
          *)
            echo "Primary config exploded" >&2
            echo "second primary line" >&2
            exit 1
            ;;
        esac
      SH
      stderr = StringIO.new

      result = call(root, files, stderr: stderr)

      expect(result.counts.values).to all(eq(0))
      expect(warning_codes(result.warnings)).to eq(['js_depcruise_failed'])
      expect(result.warnings.first.fetch(:message)).to eq('Primary config exploded')
      expect(warning_codes(result.warnings)).not_to include('js_tools_missing')
      expect(stderr.string).to include('Warning: js_depcruise_failed: Primary config exploded')
      expect(stderr.string).not_to include('second primary line')
      expect(stderr.string).not_to include('retry also failed')
    end
  end

  it 'degrades on malformed JSON without reporting missing tools' do
    make_repo do |root|
      files = %w[src/a.js src/b.js]
      files.each { |file| write_file(root, file) }
      write_depcruise(root, "#!/bin/sh\nprintf '{nope'\n")

      malformed = call(root, files)
      expect(malformed.counts.values).to all(eq(0))
      expect(warning_codes(malformed.warnings)).to eq(['js_depcruise_failed'])
      expect(warning_codes(malformed.warnings)).not_to include('js_tools_missing')
    end
  end

  it 'uses one timeout budget for the primary and --no-config dependency-cruiser attempts' do
    make_repo do |root|
      files = %w[src/a.js src/b.js]
      files.each { |file| write_file(root, file) }
      write_depcruise(root, <<~SH)
        #!/bin/sh
        case " $* " in
          *" --no-config "*) echo '{"modules":[]}' ;;
          *) exit 1 ;;
        esac
      SH
      allow(Timeout).to receive(:timeout).and_call_original
      expect(Timeout).to receive(:timeout).once.with(60).and_yield

      result = call(root, files)

      expect(warning_codes(result.warnings)).to eq(['js_depcruise_no_config'])
    end
  end

  it 'degrades on timeout' do
    make_repo do |root|
      files = %w[src/a.js src/b.js]
      files.each { |file| write_file(root, file) }
      write_depcruise(root, "#!/bin/sh\necho '{}'\n")
      allow(Timeout).to receive(:timeout).and_raise(Timeout::Error)
      stderr = StringIO.new

      result = call(root, files, js_timeout: 1, stderr: stderr)

      expect(result.counts.values).to all(eq(0))
      expect(result.warnings).to eq(['js_depcruise_timeout'])
      expect(stderr.string).to include('js_depcruise_timeout')
    end
  end
end
