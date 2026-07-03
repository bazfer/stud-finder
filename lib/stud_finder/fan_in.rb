# frozen_string_literal: true

require 'rubocop'
require 'set'

module StudFinder
  class FanIn
    Result = Struct.new(:counts, :fan_out_counts, :edges, keyword_init: true)

    PATH_ROOTS = %w[app lib test].freeze
    CLASS_OR_MODULE_TYPES = %i[class module].freeze

    def initialize(repo_path:, files:, stderr: $stderr)
      @repo_path = File.expand_path(repo_path)
      @files = files
      @stderr = stderr
    end

    def call
      constants = constant_ownership
      references = resolved_reference_sets(@files, constants)
      reverse_constants = constants.invert

      counts = @files.to_h { |file| [file, 0] }
      fan_out_counts = @files.to_h { |file| [file, 0] }
      dependents = @files.to_h { |file| [file, []] }
      dependencies = @files.to_h { |file| [file, []] }

      @files.each do |file|
        constant = constants[file]
        counts[file] = constant ? fan_in_count(file, constant, references) : 0

        references[file]&.each do |ref_constant|
          dep_file = reverse_constants[ref_constant]
          next unless dep_file && dep_file != file

          fan_out_counts[file] += 1
          dependencies[file] << dep_file
          dependents[dep_file] << file
        end
      end

      edges = @files.to_h do |file|
        [file, { dependents: dependents[file].uniq, dependencies: dependencies[file].uniq }]
      end

      Result.new(counts: counts, fan_out_counts: fan_out_counts, edges: edges)
    end

    private

    def constant_ownership
      @files.filter_map do |file|
        constant = zeitwerk_constant(file) || primary_constant(file)
        [file, constant] if constant
      end.to_h
    end

    def resolved_reference_sets(source_files, constants)
      known_constants = constants.values.to_set

      source_files.to_h do |file|
        [file, resolve_references(references_for(file), known_constants)]
      end
    end

    def resolve_references(reference_candidates, known_constants)
      reference_candidates.each_with_object(Set.new) do |candidates, resolved|
        constant = candidates.find { |candidate| known_constants.include?(candidate) }
        resolved << constant if constant
      end
    end

    def fan_in_count(file, constant, references)
      references.count do |source_file, source_refs|
        source_file != file && source_refs.include?(constant)
      end
    end

    def primary_constant(file)
      ast = parse(file)
      return unless ast

      node = ast.each_node(*CLASS_OR_MODULE_TYPES).find do |candidate|
        candidate.each_ancestor.none? { |ancestor| CLASS_OR_MODULE_TYPES.include?(ancestor.type) }
      end

      constant_name(node&.identifier)
    end

    def references_for(file)
      ast = parse(file)
      return Set.new unless ast

      ast.each_node(:const).with_object(Set.new) do |node, references|
        next if nested_const_part?(node)

        candidates = reference_candidates(node)
        references << candidates if candidates.any?
      end
    end

    def reference_candidates(node)
      name = constant_name(node)
      return [] unless name

      namespace = lexical_namespace(node)
      absolute = absolute_const_reference?(node)
      reference_candidate_cache[[namespace, name, absolute]] ||= constant_candidates(namespace, name, absolute)
    rescue StandardError => e
      @stderr.puts "Warning: fan_in_reference_resolution_failed: #{e.class}: #{e.message}"
      []
    end

    def reference_candidate_cache
      @reference_candidate_cache ||= {}
    end

    def constant_candidates(namespace, name, absolute)
      return [name] if absolute
      return [name] if namespace.nil? || namespace.empty?

      namespace.length.downto(1).map do |length|
        [namespace.first(length).join('::'), name].join('::')
      end + [name]
    end

    def lexical_namespace(node)
      node.each_ancestor.filter_map do |ancestor|
        next unless CLASS_OR_MODULE_TYPES.include?(ancestor.type)

        constant_name(ancestor.identifier)
      end.reverse
    end

    def absolute_const_reference?(node)
      parent = node.children.first
      return false unless parent
      return true if parent.cbase_type?

      parent.const_type? && absolute_const_reference?(parent)
    end

    def parse(file)
      source = File.read(File.join(@repo_path, file))
      RuboCop::ProcessedSource.new(source, RUBY_VERSION.to_f, file).ast
    rescue EncodingError, Errno::ENOENT, Parser::SyntaxError
      nil
    end

    def constant_name(node)
      return unless node&.const_type?

      node.const_name
    rescue StandardError
      nil
    end

    def nested_const_part?(node)
      node.each_ancestor.any?(&:const_type?)
    end

    def zeitwerk_constant(file)
      components = path_after_root(file)
      return unless components

      components = strip_app_concerns_namespace(components)
      components = components.reject { |component| component == 'concerns' }
      basename = components.pop&.delete_suffix('.rb')
      return if basename.nil? || basename.empty?

      constant = (components + [basename]).map { |component| camelize(component) }.join('::')
      return unless valid_constant_name?(constant)

      constant
    end

    def strip_app_concerns_namespace(components)
      concerns_index = components.index('concerns')
      return components unless concerns_index

      components[(concerns_index + 1)..] || []
    end

    def camelize(segment)
      segment.split('_').map(&:capitalize).join
    end

    def valid_constant_name?(constant)
      constant.match?(/\A[A-Z]\w*(?:::[A-Z]\w*)*\z/)
    end

    def path_after_root(file)
      components = file.split('/')
      index = components.index { |component| PATH_ROOTS.include?(component) }
      return unless index

      root = components[index]
      remaining = components[(index + 1)..]
      root == 'app' ? remaining[1..] : remaining
    end
  end
end
