# frozen_string_literal: true

module StudFinder
  # Extracts conservative Rails-style implicit constant references from Ruby ASTs.
  class RailsInference
    Inference = Struct.new(:node, :name, :absolute, keyword_init: true)

    ASSOCIATIONS = %i[belongs_to has_one has_many has_and_belongs_to_many].freeze
    COLLECTION_ASSOCIATIONS = %i[has_many has_and_belongs_to_many].freeze
    STRING_CONSTANTIZERS = %i[constantize safe_constantize].freeze
    SAFE_CLASS_BODY_WRAPPERS = %i[with_options included class_eval class_exec].freeze
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
        return association_reference(node, method_name, args)
      elsif method_name == :composed_of
        explicit_class = class_name_option(args)
        return reference(node, explicit_class) if explicit_class.is_a?(String)
      end

      nil
    end

    def string_constant_inference(node)
      receiver, method_name, = *node
      literal = string_literal(receiver)
      return unless literal

      if STRING_CONSTANTIZERS.include?(method_name)
        reference(node, literal, absolute: true)
      elsif method_name == :const_get
        reference(node, literal)
      end
    end

    def association_reference(node, method_name, args)
      association_name = symbol_name(args.first)
      return unless association_name
      return if method_name == :belongs_to && polymorphic_belongs_to?(args)

      explicit_class = class_name_option(args)
      return if explicit_class == :dynamic

      collection = COLLECTION_ASSOCIATIONS.include?(method_name)
      name = explicit_class || inferred_association_class(association_name, collection: collection)
      reference(node, name) if name
    end

    def reference(node, name, absolute: nil)
      absolute = name.start_with?('::') if absolute.nil?
      Inference.new(node: node, name: name.delete_prefix('::'), absolute: absolute)
    end

    def class_body_call?(node)
      seen_class_or_module = false

      node.each_ancestor do |ancestor|
        return false if ancestor.def_type? || ancestor.defs_type?
        return false if ancestor.block_type? && !safe_class_body_wrapper?(ancestor)

        if ancestor.class_type? || ancestor.module_type?
          seen_class_or_module = true
          break
        end
      end

      seen_class_or_module
    end

    def safe_class_body_wrapper?(node)
      send_node = node.send_node
      send_node && SAFE_CLASS_BODY_WRAPPERS.include?(send_node.method_name)
    end

    def implicit_receiver?(receiver)
      receiver.nil? || receiver.self_type?
    end

    def symbol_name(node)
      return unless node&.sym_type?

      node.value.to_s
    end

    def class_name_option(args)
      pair = option_pair(args, 'class_name')
      return unless pair

      string_literal(pair.value) || :dynamic
    end

    def polymorphic_belongs_to?(args)
      pair = option_pair(args, 'polymorphic')
      return false unless pair

      !pair.value&.false_type?
    end

    def option_pair(args, name)
      hash = args.find(&:hash_type?)
      return unless hash

      hash.pairs.find { |candidate| hash_key_name(candidate.key) == name }
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
