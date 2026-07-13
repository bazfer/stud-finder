# frozen_string_literal: true

module StudFinder
  module Warnings
    MESSAGES = {
      'coverage_flag_deprecated' => '--coverage is deprecated; use --ruby-coverage instead',
      'coverage_unavailable' => 'coverage data was not provided; coverage and interaction signals are unavailable',
      'coverage_partial' => 'coverage data did not include every scored file; missing files use uncovered risk',
      'diff_filter_empty' => 'diff filter matched no changed files; output rows are empty',
      'diff_no_scored_files' => 'diff matched no scored files; the PR may only touch unscorable files',
      'fan_in_rails_inference_failed' => 'Rails inference failed; inferred Rails references may be incomplete',
      'fan_in_reference_resolution_failed' => 'constant reference resolution failed; fan-in may be incomplete',
      'files_skipped' => 'one or more files were skipped during analysis',
      'git_error' => 'git command failed while computing temporal coupling; coupling is unavailable',
      'git_not_found' => 'git was not found in PATH while computing temporal coupling; coupling is unavailable',
      'js_depcruise_failed' => 'dependency-cruiser failed; JavaScript/TypeScript fan-in is unavailable',
      'js_depcruise_no_config' => 'dependency-cruiser config failed; retried with --no-config, so ' \
                                  'JavaScript/TypeScript fan-in may be undercounted',
      'js_depcruise_timeout' => 'dependency-cruiser timed out; JavaScript/TypeScript fan-in is unavailable',
      'js_eslint_failed' => 'ESLint failed for at least one batch; JavaScript/TypeScript complexity may be incomplete',
      'js_eslint_malformed' => 'ESLint produced malformed JSON for at least one batch; JavaScript/TypeScript ' \
                               'complexity may be incomplete',
      'js_eslint_missing' => 'ESLint was not found; JavaScript/TypeScript complexity is unavailable',
      'js_eslint_timeout' => 'ESLint timed out for at least one batch; JavaScript/TypeScript complexity may be ' \
                             'incomplete',
      'js_tools_missing' => 'Node.js or dependency-cruiser was not found; JavaScript/TypeScript fan-in is unavailable',
      'js_ts_parser_missing' => 'TypeScript files were present but @typescript-eslint/parser was not found; ' \
                                'TypeScript complexity may be incomplete',
      'shallow_clone_newness_disabled' => 'shallow git clone detected; newness rules disabled (use fetch-depth: 0)',
      'shallow_clone_unshallow_failed' => 'auto-unshallow failed; evidence unavailable (use full git history)',
      'small_repo' => 'repo has fewer files than --min-files; results may be noisy',
      'temporal_coupling_bulk_commits_skipped' => 'one or more bulk commits were skipped while computing temporal ' \
                                                  'coupling',
      'zero_churn_majority' => 'most files have zero churn in the selected window; churn may be less informative'
    }.freeze

    INSUFFICIENT_DISPERSION_MESSAGE = lambda do |signal|
      "every file has the same non-zero raw #{signal} value, so its percentile-ranked contribution collapsed to 0.0"
    end
    private_constant :INSUFFICIENT_DISPERSION_MESSAGE

    module_function

    def normalize(items)
      Array(items).map { |item| normalize_one(item) }.uniq { |warning| warning.fetch(:code) }
    end

    def normalize_one(item)
      code, explicit_message = warning_parts(item)
      { code: code, message: message_for(code, explicit_message, item) }
    end

    def message_for(code, explicit_message = nil, item = nil)
      return explicit_message.to_s unless explicit_message.to_s.empty?

      if code.start_with?('insufficient_dispersion_')
        signal = code.delete_prefix('insufficient_dispersion_')
        return INSUFFICIENT_DISPERSION_MESSAGE.call(signal)
      end

      if code == 'temporal_coupling_bulk_commits_skipped' && item.respond_to?(:fetch)
        count = item.fetch(:count, nil) || item.fetch('count', nil)
        max = item.fetch(:max_commit_files, nil) || item.fetch('max_commit_files', nil)
        return "#{MESSAGES.fetch(code)} (#{count} skipped; max_commit_files=#{max})" if count && max
      end

      MESSAGES.fetch(code) { code.tr('_', ' ') }
    end

    def warning_parts(item)
      if item.respond_to?(:fetch) && (item.key?(:code) || item.key?('code'))
        code = item.fetch(:code, nil) || item.fetch('code')
        message = item.fetch(:message, nil) || item.fetch('message', nil)
        [code.to_s, message]
      else
        [item.to_s, nil]
      end
    end
    private_class_method :warning_parts
  end
end
