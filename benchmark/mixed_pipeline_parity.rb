# frozen_string_literal: true

require 'tmpdir'
require 'fileutils'
require 'stringio'
require 'json'
require_relative '../lib/stud_finder/session'

clock = Time.now
Time.define_singleton_method(:now) { clock }
# Isolated fixture only; the supplied module installation is read, never edited.
modules = File.expand_path(ENV.fetch('STUD_NODE_MODULES'))
frontend_modules = ENV.fetch('STUD_FRONTEND_NODE_MODULES', nil)
results = []
Dir.mktmpdir('stud-pipeline-') do |root| # rubocop:disable Metrics/BlockLength
  root = File.realpath(root)
  system('git', 'init', '-q', root)
  system('git', '-C', root, 'config', 'user.name', 'Pipeline Test')
  system('git', '-C', root, 'config', 'user.email', 'pipeline@example.invalid')
  FileUtils.mkdir_p(File.join(root, 'src'))
  FileUtils.mkdir_p(File.join(root, 'frontend'))
  File.symlink(modules, File.join(root, 'node_modules'))
  File.symlink(frontend_modules, File.join(root, 'frontend/node_modules')) if frontend_modules
  6.times { |i| File.write(File.join(root, "model_#{i}.rb"), "class Model#{i}; end\n") }
  sources = {
    'a.ts' => "import { b } from './b'; export const a = (x: boolean) => x ? b() : 0;",
    'b.ts' => 'export const b = () => 1;',
    'c.ts' => "export { a } from './a';",
    'd.ts' => "export const load = () => import('./b');",
    'e.ts' => "import { b } from '@/b'; import { b as target } from 'tools/target'; " \
              'export const e = b; export const t = target;',
    'f.js' => "const b = require('./b'); export const f = b;",
    'target.ts' => 'export const b = () => 2;',
    'pkg/index.ts' => 'export const b = () => 3;',
    'pkg/alternate.ts' => 'export const b = () => 4;'
  }
  sources.each do |file, text|
    FileUtils.mkdir_p(File.dirname(File.join(root, 'src', file)))
    File.write(File.join(root, 'src', file), text)
  end
  tsconfig = { compilerOptions: { target: 'ES2020', module: 'ESNext', moduleResolution: 'bundler', baseUrl: '.',
                                  paths: { '@/*' => ['src/*'], 'tools/*' => ['src/*'] } }, include: ['src'] }
  File.write(File.join(root, 'tsconfig.json'), JSON.generate(tsconfig))
  config = { options: { doNotFollow: { path: 'node_modules' }, exclude: { path: 'node_modules' },
                        tsConfig: { fileName: File.join(root, 'tsconfig.json') } } }
  File.write(File.join(root, '.dependency-cruiser.json'), JSON.generate(config))
  system('git', '-C', root, 'add', '*.rb', 'src', 'tsconfig.json', '.dependency-cruiser.json')
  system('git', '-C', root, 'commit', '-qm', 'initial')
  args = [root, '--no-auto-unshallow']
  cache = File.join(root, '.stud-finder-cache')
  diagnostics = StringIO.new
  session = StudFinder::Session.new(args + ['--cache-dir', cache], stderr: diagnostics)
  verify = lambda do |label, extra = []|
    out = StringIO.new
    status = StudFinder::CLI.start(args + extra + ['--output', 'json'], stdout: out, stderr: StringIO.new)
    raise "oracle failed: #{label}" unless status.zero?

    oracle = JSON.parse(out.string)
    if oracle['warnings'].any? { |w| w['code'] == 'js_depcruise_failed' }
      _, err, = Open3.capture3(File.join(root, 'node_modules/.bin/depcruise'), '--output-type', 'json', '.',
                               chdir: root)
      warn err
    end
    actual = session.scan(extra_args: extra)
    [oracle, actual].each { |report| report['meta'].delete('analyzed_at') }
    unless oracle == actual
      File.write(File.join(__dir__, 'integration-actual.json'), JSON.pretty_generate(actual))
      File.write(File.join(__dir__, 'integration-oracle.json'), JSON.pretty_generate(oracle))
      raise "parity failed: #{label}"
    end
    results << label
    warn "PASS #{label}"
  end
  begin
    verify.call('initial mixed pipeline with aliases, dynamic imports and CommonJS')
    verify.call('warm cache inside repository')
    raise 'native graph cache not used' unless File.file?(File.join(cache, 'js-graph.json'))

    file = File.join(root, 'src/a.ts')
    stamp = File.mtime(file)
    File.write(file, sources['a.ts'].sub("'./b'", "'./target'"))
    File.utime(stamp, stamp, file)
    verify.call('changed import with preserved mtime')
    File.write(file, sources['a.ts'].sub("'./b'", "'./pkg'"))
    verify.call('directory import before package metadata')
    package = File.join(root, 'src/pkg/package.json')
    File.write(package, JSON.generate(main: 'alternate.ts'))
    verify.call('new package metadata changes resolution without source edits')
    File.write(package, JSON.generate(main: 'index.ts'))
    verify.call('edited package metadata changes resolution')
    File.delete(package)
    verify.call('deleted package metadata restores directory resolution')
    File.rename(File.join(root, 'src/target.ts'), File.join(root, 'src/renamed.ts'))
    File.write(file, sources['a.ts'].sub("'./b'", "'./renamed'"))
    verify.call('rename and import update')
    File.delete(File.join(root, 'src/renamed.ts'))
    verify.call('deleted imported module')
    File.write(File.join(root, 'src/renamed.ts'), 'export const b = () => 5;')
    verify.call('new module resolves previously missing import')
    tsconfig[:compilerOptions][:paths] = { '@/*' => ['src/renamed'] }
    File.write(File.join(root, 'tsconfig.json'), JSON.generate(tsconfig))
    verify.call('TypeScript alias configuration change')
    ruby_coverage = File.join(root, 'ruby.info')
    js_coverage = File.join(root, 'js.info')
    File.write(ruby_coverage, "SF:model_0.rb\nDA:1,1\nend_of_record\n")
    File.write(js_coverage, "SF:src/a.ts\nDA:1,1\nend_of_record\n")
    extra = ['--ruby-coverage', ruby_coverage, '--js-coverage', js_coverage]
    verify.call('coverage inputs and coverage-weighted scoring', extra)
    File.write(js_coverage, "SF:src/a.ts\nDA:1,0\nend_of_record\n")
    verify.call('coverage update changes scores', extra)
    session.close
    damaged = JSON.parse(File.read(File.join(cache, 'js-graph.json')))
    damaged['modules'] = 'corrupt'
    File.write(File.join(cache, 'js-graph.json'), JSON.generate(damaged))
    session = StudFinder::Session.new(args + ['--cache-dir', cache], stderr: diagnostics)
    verify.call('new session with corrupt graph cache')
    raise diagnostics.string if diagnostics.string.match?(/worker (fallback|unsupported)/)

    config[:options][:focus] = 'src/a'
    File.write(File.join(root, '.dependency-cruiser.json'), JSON.generate(config))
    verify.call('unsupported graph filter uses full backend')
  ensure
    session.close
  end
end
puts JSON.pretty_generate(passed: results)
