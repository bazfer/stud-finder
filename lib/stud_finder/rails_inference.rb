# frozen_string_literal: true

module StudFinder
  # Extracts conservative Rails-style implicit constant references from Ruby ASTs.
  class RailsInference
    Inference = Struct.new(:node, :name, :absolute, keyword_init: true)

    ASSOCIATIONS = %i[belongs_to has_one has_many has_and_belongs_to_many].freeze
    COLLECTION_ASSOCIATIONS = %i[has_many has_and_belongs_to_many].freeze
    STRING_CONSTANTIZERS = %i[constantize safe_constantize].freeze
    IRREGULAR_SINGULARS = {
      'people' => 'person',
      'children' => 'child',
      'men' => 'man',
      'women' => 'woman',
      'mice' => 'mouse',
      'geese' => 'goose'
    }.freeze

    def initialize(ast)
      @ast = ast
    end

    def call
      return [] unless @ast

      @ast.each_node(:send).filter_map do |node|
        association_inference(node) || string_constant_inference(node)
      end
    end

    private

    def association_inference(node)
      _receiver, method_name, *args = *node
      return unless class_body_call?(node)
      return unless implicit_receiver?(node.receiver)

      if ASSOCIATIONS.include?(method_name)
        association_name = symbol_name(args.first)
        return unless association_name

        explicit_class = class_name_option(args)
        return if explicit_class == :dynamic

        collection = COLLECTION_ASSOCIATIONS.include?(method_name)
        name = explicit_class || inferred_association_class(association_name, collection: collection)
        return reference(node, name) if name
      elsif method_name == :composed_of
        explicit_class = class_name_option(args)
        return reference(node, explicit_class) if explicit_class.is_a?(String)
      end

      nil
    end

    def string_constant_inference(node)
      receiver, method_name, *args = *node
      if STRING_CONSTANTIZERS.include?(method_name)
        literal = terminal_string_literal(receiver)
        return reference(node, literal) if literal
      elsif method_name == :const_get
        literal = string_literal(args.first)
        return reference(node, literal) if literal
      end

      nil
    end

    def reference(node, name)
      absolute = name.start_with?('::')
      Inference.new(node: node, name: absolute ? name.delete_prefix('::') : name, absolute: absolute)
    end

    def class_body_call?(node)
      seen_class_or_module = false

      node.each_ancestor do |ancestor|
        return false if ancestor.def_type? || ancestor.defs_type? || ancestor.block_type?

        if ancestor.class_type? || ancestor.module_type?
          seen_class_or_module = true
          break
        end
      end

      seen_class_or_module
    end

    def implicit_receiver?(receiver)
      receiver.nil? || receiver.self_type?
    end

    def symbol_name(node)
      return unless node&.sym_type?

      node.value.to_s
    end

    def class_name_option(args)
      hash = args.find(&:hash_type?)
      return unless hash

      pair = hash.pairs.find { |candidate| hash_key_name(candidate.key) == 'class_name' }
      return unless pair

      string_literal(pair.value) || :dynamic
    end

    def hash_key_name(node)
      return node.value.to_s if node&.sym_type?
      return node.value if node&.str_type?

      nil
    end

    def string_literal(node)
      return node.value if node&.str_type?

      nil
    end

    def terminal_string_literal(node)
      current = node
      current = current.receiver while current&.send_type?
      string_literal(current)
    end

    def inferred_association_class(name, collection:)
      source = collection ? singularize(name) : name
      camelize(source)
    end

    # Minimal heuristic only, not Rails inflection. Wrong guesses simply do not
    # resolve to owned constants in FanIn.
    def singularize(word)
      lower = word.downcase
      return IRREGULAR_SINGULARS.fetch(lower) if IRREGULAR_SINGULARS.key?(lower)

      case word
      when /(ses|xes|zes|ches|shes)\z/
        word.delete_suffix('es')
      when /ies\z/
        "#{word[0...-3]}y"
      when /s\z/
        word.delete_suffix('s')
      else
        word
      end
    end

    def camelize(segment)
      segment.split('_').map(&:capitalize).join
    end
  end
end
