# frozen_string_literal: true

module StudFinder
  # Session-owned directory inventories. File contents are never cached here.
  # Directory replacement and entry changes invalidate inode/mtime/ctime stamps.
  class DirectoryIndex
    def initialize
      @entries = {}
    end

    def entries(path, stamp)
      cached = @entries[path]
      return cached[:entries] if cached && cached[:stamp] == stamp

      entries = Dir.children(path).sort.map do |name|
        file = File.join(path, name)
        stat = File.lstat(file)
        kind = if stat.directory?
                 :directory
               elsif stat.symlink?
                 :symlink
               elsif stat.file?
                 :file
               else
                 :other
               end
        [file, kind]
      end
      @entries[path] = { stamp: stamp, entries: entries }
      entries
    end

    def retain(paths)
      @entries.delete_if { |path, _| !paths.key?(path) }
    end
  end
end
