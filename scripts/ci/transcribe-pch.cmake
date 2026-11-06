# Parse shared C++ standard-library headers once for the transcription library.
# Defer until transcribe's CMakeLists has created the target.
function(bespoke_transcribe_pch)
    target_precompile_headers(transcribe PRIVATE
        "$<$<COMPILE_LANGUAGE:CXX>:<algorithm$<ANGLE-R>>"
        "$<$<COMPILE_LANGUAGE:CXX>:<memory$<ANGLE-R>>"
        "$<$<COMPILE_LANGUAGE:CXX>:<string$<ANGLE-R>>"
        "$<$<COMPILE_LANGUAGE:CXX>:<unordered_map$<ANGLE-R>>"
        "$<$<COMPILE_LANGUAGE:CXX>:<vector$<ANGLE-R>>")
endfunction()
cmake_language(DEFER CALL bespoke_transcribe_pch)
