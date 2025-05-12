module Util
    def self.split_message(message, max_length = 160)
        return [message] if message.length <= max_length
        
        chunks = []
        current_chunk = ""
        words = message.split(" ")
        
        words.each do |word|
          if (current_chunk + " " + word).length <= max_length - 5 # Reserve space for "1/2: "
            current_chunk += " " + word
          else
            chunks << current_chunk.strip
            current_chunk = word
          end
        end
        
        chunks << current_chunk.strip if current_chunk.length > 0
        
        chunks.map.with_index(1) do |chunk, index|
          "#{index}/#{chunks.length}: #{chunk}"
        end
    end
end
