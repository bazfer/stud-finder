# frozen_string_literal: true

require_relative 'js_worker'

module StudFinder
  module JsWorkerClient
    private

    def worker_call
      payload = @worker.call(root: @repo_path, operation: worker_operation, files: @files,
                             timeout: @js_timeout, cache_dir: @cache_dir, config: @js_config)
      if payload['unsupported']
        @worker.close if payload['restart']
        @stderr.puts "JavaScript worker unsupported: #{payload['reason']}" if payload['reason']
        return fallback_call
      end
      worker_result(payload)
    rescue JsWorker::Error => e
      @stderr.puts "JavaScript worker fallback: #{e.message}"
      fallback_call
    end

    def fallback_call
      @worker = nil
      call
    end
  end
end
