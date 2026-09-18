# frozen_string_literal: true

require 'digest'
require 'fileutils'
require 'json'
require 'tempfile'

module StudFinder
  # Disposable source facts, never executable Ruby objects. Concurrent writers
  # may lose cache hits, but atomic replacement and fingerprints prevent mixing
  # records from different source versions.
  class FactCache
    SCHEMA = 1

    def initialize(directory:, namespace:, stderr: $stderr)
      @directory = File.expand_path(directory)
      @path = File.join(@directory, "facts-#{Digest::SHA256.hexdigest(namespace)}.json")
      @stderr = stderr
      @dirty = false
      @entries = read_entries
    end

    def fetch(key, fingerprint)
      entry = @entries[key]
      entry['value'] if entry.is_a?(Hash) && entry['fingerprint'] == fingerprint
    end

    def store(key, fingerprint, value)
      @entries[key] = { 'fingerprint' => fingerprint, 'value' => value }
      @dirty = true
    end

    def retain(keys)
      wanted = keys.to_h { |key| [key, true] }
      @entries.delete_if do |key, _value|
        @dirty = true unless wanted[key]
        !wanted[key]
      end
    end

    def save
      return unless @dirty

      FileUtils.mkdir_p(@directory)
      Tempfile.create(['facts-', '.tmp'], @directory) do |file|
        file.write(JSON.generate('schema' => SCHEMA, 'entries' => @entries))
        file.flush
        file.fsync
        File.rename(file.path, @path)
      end
      @dirty = false
    rescue SystemCallError, IOError => e
      @stderr.puts "Warning: fact cache write failed: #{e.class}"
    end

    private

    def read_entries
      payload = JSON.parse(File.read(@path))
      return {} unless payload.is_a?(Hash) && payload['schema'] == SCHEMA && payload['entries'].is_a?(Hash)

      payload['entries']
    rescue JSON::ParserError, SystemCallError, IOError
      {}
    end
  end
end
