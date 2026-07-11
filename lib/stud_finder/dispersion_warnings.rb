# frozen_string_literal: true

module StudFinder
  module DispersionWarnings
    module_function

    def build(files:, pcts:, raw_sources:)
      raw_sources.filter_map do |signal, raw_source|
        "insufficient_dispersion_#{signal}" if insufficient_dispersion?(files, pcts.fetch(signal), raw_source)
      end
    end

    def insufficient_dispersion?(files, pct_map, raw_source)
      values = files.map { |file| pct_map.fetch(file, 0.0).to_f }
      raw_values = files.map { |file| raw_source.fetch(file, 0).to_f }

      values.any? && values.all?(&:zero?) && raw_values.any? { |value| !value.zero? }
    end
  end
end
