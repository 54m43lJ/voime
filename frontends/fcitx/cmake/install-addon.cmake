# Install the Voime addon into the Fcitx5 system addon directories.
#
# These helpers intentionally refuse a user-prefix fallback: the product only
# supports system-level addon installation, and the core installer asks for
# sudo for this step.

include_guard(GLOBAL)

function(voime_install_addon target)
    if(NOT TARGET ${target})
        message(FATAL_ERROR "voime_install_addon: unknown target ${target}")
    endif()
    if(DEFINED FCITX5_LIBDIR AND NOT FCITX5_LIBDIR STREQUAL "")
        install(TARGETS ${target} LIBRARY DESTINATION ${FCITX5_LIBDIR}/fcitx5)
    else()
        install(TARGETS ${target} LIBRARY DESTINATION ${CMAKE_INSTALL_LIBDIR}/fcitx5)
    endif()
    install(FILES "${CMAKE_CURRENT_BINARY_DIR}/voime.conf"
            DESTINATION ${CMAKE_INSTALL_DATADIR}/fcitx5/addon)
    install(CODE "file(REMOVE \"${CMAKE_INSTALL_FULL_DATADIR}/fcitx5/inputmethod/voime.conf\")")
endfunction()

function(voime_check_addon_abi target)
    if(NOT TARGET ${target})
        message(FATAL_ERROR "voime_check_addon_abi: unknown target ${target}")
    endif()
    add_custom_command(TARGET ${target} POST_BUILD
        COMMAND ${CMAKE_COMMAND} -E echo
                "Voime addon installed; restart Fcitx5 and run `voime doctor` to verify the system ABI")
endfunction()
