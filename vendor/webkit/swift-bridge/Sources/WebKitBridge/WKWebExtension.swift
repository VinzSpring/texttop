// WKWebExtension was introduced in macOS 15.4 and is not available in the
// Xcode 15 / macOS 14 SDK.  All functions are stubbed to return WK_UNSUPPORTED
// so the Swift bridge compiles; the Rust side already checks availability
// before calling any of these entry points.
import Foundation

@_cdecl("wk_web_extension_copy_constants_json")
public func wk_web_extension_copy_constants_json() -> UnsafeMutablePointer<CChar>? { nil }

@_cdecl("wk_web_extension_create_with_resource_base_url")
public func wk_web_extension_create_with_resource_base_url(_ path: UnsafePointer<CChar>?, _ outExtension: UnsafeMutablePointer<UnsafeMutableRawPointer?>?, _ outErr: UnsafeMutablePointer<UnsafeMutablePointer<CChar>?>?) -> Int32 { WK_UNSUPPORTED }

@_cdecl("wk_web_extension_create_with_app_extension_bundle")
public func wk_web_extension_create_with_app_extension_bundle(_ path: UnsafePointer<CChar>?, _ outExtension: UnsafeMutablePointer<UnsafeMutableRawPointer?>?, _ outErr: UnsafeMutablePointer<UnsafeMutablePointer<CChar>?>?) -> Int32 { WK_UNSUPPORTED }

@_cdecl("wk_web_extension_release")
public func wk_web_extension_release(_ ptr: UnsafeMutableRawPointer?) {}

@_cdecl("wk_web_extension_copy_summary_json")
public func wk_web_extension_copy_summary_json(_ ptr: UnsafeMutableRawPointer?) -> UnsafeMutablePointer<CChar>? { nil }

@_cdecl("wk_web_extension_supports_manifest_version")
public func wk_web_extension_supports_manifest_version(_ ptr: UnsafeMutableRawPointer?, _ manifestVersion: Double) -> Bool { false }

@_cdecl("wk_web_extension_match_pattern_register_custom_url_scheme")
public func wk_web_extension_match_pattern_register_custom_url_scheme(_ scheme: UnsafePointer<CChar>?, _ outErr: UnsafeMutablePointer<UnsafeMutablePointer<CChar>?>?) -> Int32 { WK_UNSUPPORTED }

@_cdecl("wk_web_extension_match_pattern_all_urls")
public func wk_web_extension_match_pattern_all_urls() -> UnsafeMutableRawPointer? { nil }

@_cdecl("wk_web_extension_match_pattern_all_hosts_and_schemes")
public func wk_web_extension_match_pattern_all_hosts_and_schemes() -> UnsafeMutableRawPointer? { nil }

@_cdecl("wk_web_extension_match_pattern_with_string")
public func wk_web_extension_match_pattern_with_string(_ pattern: UnsafePointer<CChar>?, _ outPattern: UnsafeMutablePointer<UnsafeMutableRawPointer?>?, _ outErr: UnsafeMutablePointer<UnsafeMutablePointer<CChar>?>?) -> Int32 { WK_UNSUPPORTED }

@_cdecl("wk_web_extension_match_pattern_with_components")
public func wk_web_extension_match_pattern_with_components(_ scheme: UnsafePointer<CChar>?, _ host: UnsafePointer<CChar>?, _ path: UnsafePointer<CChar>?, _ outPattern: UnsafeMutablePointer<UnsafeMutableRawPointer?>?, _ outErr: UnsafeMutablePointer<UnsafeMutablePointer<CChar>?>?) -> Int32 { WK_UNSUPPORTED }

@_cdecl("wk_web_extension_match_pattern_release")
public func wk_web_extension_match_pattern_release(_ ptr: UnsafeMutableRawPointer?) {}

@_cdecl("wk_web_extension_match_pattern_copy_summary_json")
public func wk_web_extension_match_pattern_copy_summary_json(_ ptr: UnsafeMutableRawPointer?) -> UnsafeMutablePointer<CChar>? { nil }

@_cdecl("wk_web_extension_match_pattern_matches_url")
public func wk_web_extension_match_pattern_matches_url(_ ptr: UnsafeMutableRawPointer?, _ url: UnsafePointer<CChar>?, _ options: UInt64) -> Bool { false }

@_cdecl("wk_web_extension_match_pattern_matches_pattern")
public func wk_web_extension_match_pattern_matches_pattern(_ ptr: UnsafeMutableRawPointer?, _ otherPtr: UnsafeMutableRawPointer?, _ options: UInt64) -> Bool { false }

@_cdecl("wk_web_extension_controller_configuration_default")
public func wk_web_extension_controller_configuration_default() -> UnsafeMutableRawPointer? { nil }

@_cdecl("wk_web_extension_controller_configuration_nonpersistent")
public func wk_web_extension_controller_configuration_nonpersistent() -> UnsafeMutableRawPointer? { nil }

@_cdecl("wk_web_extension_controller_configuration_with_identifier")
public func wk_web_extension_controller_configuration_with_identifier(_ identifier: UnsafePointer<CChar>?, _ outConfiguration: UnsafeMutablePointer<UnsafeMutableRawPointer?>?, _ outErr: UnsafeMutablePointer<UnsafeMutablePointer<CChar>?>?) -> Int32 { WK_UNSUPPORTED }

@_cdecl("wk_web_extension_controller_configuration_release")
public func wk_web_extension_controller_configuration_release(_ ptr: UnsafeMutableRawPointer?) {}

@_cdecl("wk_web_extension_controller_configuration_copy_summary_json")
public func wk_web_extension_controller_configuration_copy_summary_json(_ ptr: UnsafeMutableRawPointer?) -> UnsafeMutablePointer<CChar>? { nil }

@_cdecl("wk_web_extension_controller_configuration_set_webview_configuration")
public func wk_web_extension_controller_configuration_set_webview_configuration(_ ptr: UnsafeMutableRawPointer?, _ configPtr: UnsafeMutableRawPointer?) {}

@_cdecl("wk_web_extension_controller_configuration_copy_webview_configuration")
public func wk_web_extension_controller_configuration_copy_webview_configuration(_ ptr: UnsafeMutableRawPointer?) -> UnsafeMutableRawPointer? { nil }

@_cdecl("wk_web_extension_controller_configuration_set_default_website_data_store")
public func wk_web_extension_controller_configuration_set_default_website_data_store(_ ptr: UnsafeMutableRawPointer?, _ storePtr: UnsafeMutableRawPointer?) {}

@_cdecl("wk_web_extension_controller_configuration_copy_default_website_data_store")
public func wk_web_extension_controller_configuration_copy_default_website_data_store(_ ptr: UnsafeMutableRawPointer?) -> UnsafeMutableRawPointer? { nil }

@_cdecl("wk_web_extension_controller_new")
public func wk_web_extension_controller_new() -> UnsafeMutableRawPointer? { nil }

@_cdecl("wk_web_extension_controller_with_configuration")
public func wk_web_extension_controller_with_configuration(_ configurationPtr: UnsafeMutableRawPointer?) -> UnsafeMutableRawPointer? { nil }

@_cdecl("wk_web_extension_controller_release")
public func wk_web_extension_controller_release(_ ptr: UnsafeMutableRawPointer?) {}

@_cdecl("wk_web_extension_controller_copy_configuration")
public func wk_web_extension_controller_copy_configuration(_ ptr: UnsafeMutableRawPointer?) -> UnsafeMutableRawPointer? { nil }

@_cdecl("wk_web_extension_controller_load_context")
public func wk_web_extension_controller_load_context(_ ptr: UnsafeMutableRawPointer?, _ contextPtr: UnsafeMutableRawPointer?, _ outErr: UnsafeMutablePointer<UnsafeMutablePointer<CChar>?>?) -> Int32 { WK_UNSUPPORTED }

@_cdecl("wk_web_extension_controller_unload_context")
public func wk_web_extension_controller_unload_context(_ ptr: UnsafeMutableRawPointer?, _ contextPtr: UnsafeMutableRawPointer?, _ outErr: UnsafeMutablePointer<UnsafeMutablePointer<CChar>?>?) -> Int32 { WK_UNSUPPORTED }

@_cdecl("wk_web_extension_controller_copy_context_for_extension")
public func wk_web_extension_controller_copy_context_for_extension(_ ptr: UnsafeMutableRawPointer?, _ extensionPtr: UnsafeMutableRawPointer?) -> UnsafeMutableRawPointer? { nil }

@_cdecl("wk_web_extension_controller_copy_context_for_url")
public func wk_web_extension_controller_copy_context_for_url(_ ptr: UnsafeMutableRawPointer?, _ url: UnsafePointer<CChar>?) -> UnsafeMutableRawPointer? { nil }

@_cdecl("wk_web_extension_controller_copy_all_data_types_json")
public func wk_web_extension_controller_copy_all_data_types_json() -> UnsafeMutablePointer<CChar>? { nil }

@_cdecl("wk_web_extension_controller_copy_data_records_json")
public func wk_web_extension_controller_copy_data_records_json(_ ptr: UnsafeMutableRawPointer?, _ dataTypesJson: UnsafePointer<CChar>?, _ outJson: UnsafeMutablePointer<UnsafeMutablePointer<CChar>?>?, _ outErr: UnsafeMutablePointer<UnsafeMutablePointer<CChar>?>?) -> Int32 { WK_UNSUPPORTED }

@_cdecl("wk_web_extension_controller_copy_data_record_json_for_context")
public func wk_web_extension_controller_copy_data_record_json_for_context(_ ptr: UnsafeMutableRawPointer?, _ dataTypesJson: UnsafePointer<CChar>?, _ contextPtr: UnsafeMutableRawPointer?, _ outJson: UnsafeMutablePointer<UnsafeMutablePointer<CChar>?>?, _ outErr: UnsafeMutablePointer<UnsafeMutablePointer<CChar>?>?) -> Int32 { WK_UNSUPPORTED }

@_cdecl("wk_web_extension_controller_remove_data_for_identifiers")
public func wk_web_extension_controller_remove_data_for_identifiers(_ ptr: UnsafeMutableRawPointer?, _ dataTypesJson: UnsafePointer<CChar>?, _ identifiersJson: UnsafePointer<CChar>?, _ outErr: UnsafeMutablePointer<UnsafeMutablePointer<CChar>?>?) -> Int32 { WK_UNSUPPORTED }

@_cdecl("wk_web_extension_context_new_for_extension")
public func wk_web_extension_context_new_for_extension(_ extensionPtr: UnsafeMutableRawPointer?) -> UnsafeMutableRawPointer? { nil }

@_cdecl("wk_web_extension_context_release")
public func wk_web_extension_context_release(_ ptr: UnsafeMutableRawPointer?) {}

@_cdecl("wk_web_extension_context_copy_summary_json")
public func wk_web_extension_context_copy_summary_json(_ ptr: UnsafeMutableRawPointer?) -> UnsafeMutablePointer<CChar>? { nil }

@_cdecl("wk_web_extension_context_set_base_url")
public func wk_web_extension_context_set_base_url(_ ptr: UnsafeMutableRawPointer?, _ url: UnsafePointer<CChar>?, _ outErr: UnsafeMutablePointer<UnsafeMutablePointer<CChar>?>?) -> Int32 { WK_UNSUPPORTED }

@_cdecl("wk_web_extension_context_set_unique_identifier")
public func wk_web_extension_context_set_unique_identifier(_ ptr: UnsafeMutableRawPointer?, _ identifier: UnsafePointer<CChar>?, _ outErr: UnsafeMutablePointer<UnsafeMutablePointer<CChar>?>?) -> Int32 { WK_UNSUPPORTED }

@_cdecl("wk_web_extension_context_set_inspectable")
public func wk_web_extension_context_set_inspectable(_ ptr: UnsafeMutableRawPointer?, _ value: Bool) {}

@_cdecl("wk_web_extension_context_set_inspection_name")
public func wk_web_extension_context_set_inspection_name(_ ptr: UnsafeMutableRawPointer?, _ name: UnsafePointer<CChar>?) {}

@_cdecl("wk_web_extension_context_set_unsupported_apis_json")
public func wk_web_extension_context_set_unsupported_apis_json(_ ptr: UnsafeMutableRawPointer?, _ apisJson: UnsafePointer<CChar>?) {}

@_cdecl("wk_web_extension_context_set_requested_optional_access_to_all_hosts")
public func wk_web_extension_context_set_requested_optional_access_to_all_hosts(_ ptr: UnsafeMutableRawPointer?, _ value: Bool) {}

@_cdecl("wk_web_extension_context_set_access_to_private_data")
public func wk_web_extension_context_set_access_to_private_data(_ ptr: UnsafeMutableRawPointer?, _ value: Bool) {}

@_cdecl("wk_web_extension_context_copy_webview_configuration")
public func wk_web_extension_context_copy_webview_configuration(_ ptr: UnsafeMutableRawPointer?) -> UnsafeMutableRawPointer? { nil }

@_cdecl("wk_web_extension_context_has_permission")
public func wk_web_extension_context_has_permission(_ ptr: UnsafeMutableRawPointer?, _ permission: UnsafePointer<CChar>?) -> Bool { false }

@_cdecl("wk_web_extension_context_has_access_to_url")
public func wk_web_extension_context_has_access_to_url(_ ptr: UnsafeMutableRawPointer?, _ url: UnsafePointer<CChar>?) -> Bool { false }

@_cdecl("wk_web_extension_context_permission_status_for_permission")
public func wk_web_extension_context_permission_status_for_permission(_ ptr: UnsafeMutableRawPointer?, _ permission: UnsafePointer<CChar>?) -> Int64 { 0 }

@_cdecl("wk_web_extension_context_set_permission_status_for_permission")
public func wk_web_extension_context_set_permission_status_for_permission(_ ptr: UnsafeMutableRawPointer?, _ status: Int64, _ permission: UnsafePointer<CChar>?, _ outErr: UnsafeMutablePointer<UnsafeMutablePointer<CChar>?>?) -> Int32 { WK_UNSUPPORTED }

@_cdecl("wk_web_extension_context_permission_status_for_url")
public func wk_web_extension_context_permission_status_for_url(_ ptr: UnsafeMutableRawPointer?, _ url: UnsafePointer<CChar>?) -> Int64 { 0 }

@_cdecl("wk_web_extension_context_set_permission_status_for_url")
public func wk_web_extension_context_set_permission_status_for_url(_ ptr: UnsafeMutableRawPointer?, _ status: Int64, _ url: UnsafePointer<CChar>?, _ outErr: UnsafeMutablePointer<UnsafeMutablePointer<CChar>?>?) -> Int32 { WK_UNSUPPORTED }

@_cdecl("wk_web_extension_context_permission_status_for_match_pattern")
public func wk_web_extension_context_permission_status_for_match_pattern(_ ptr: UnsafeMutableRawPointer?, _ patternPtr: UnsafeMutableRawPointer?) -> Int64 { 0 }

@_cdecl("wk_web_extension_context_set_permission_status_for_match_pattern")
public func wk_web_extension_context_set_permission_status_for_match_pattern(_ ptr: UnsafeMutableRawPointer?, _ status: Int64, _ patternPtr: UnsafeMutableRawPointer?, _ outErr: UnsafeMutablePointer<UnsafeMutablePointer<CChar>?>?) -> Int32 { WK_UNSUPPORTED }

@_cdecl("wk_web_extension_context_load_background_content")
public func wk_web_extension_context_load_background_content(_ ptr: UnsafeMutableRawPointer?, _ outErr: UnsafeMutablePointer<UnsafeMutablePointer<CChar>?>?) -> Int32 { WK_UNSUPPORTED }

@_cdecl("wk_web_extension_context_copy_default_action_json")
public func wk_web_extension_context_copy_default_action_json(_ ptr: UnsafeMutableRawPointer?) -> UnsafeMutablePointer<CChar>? { nil }

@_cdecl("wk_web_extension_context_perform_default_action")
public func wk_web_extension_context_perform_default_action(_ ptr: UnsafeMutableRawPointer?) {}

@_cdecl("wk_web_extension_context_copy_commands_json")
public func wk_web_extension_context_copy_commands_json(_ ptr: UnsafeMutableRawPointer?) -> UnsafeMutablePointer<CChar>? { nil }

@_cdecl("wk_web_extension_context_perform_command_for_identifier")
public func wk_web_extension_context_perform_command_for_identifier(_ ptr: UnsafeMutableRawPointer?, _ identifier: UnsafePointer<CChar>?, _ outErr: UnsafeMutablePointer<UnsafeMutablePointer<CChar>?>?) -> Int32 { WK_UNSUPPORTED }

@_cdecl("wk_web_extension_message_port_release")
public func wk_web_extension_message_port_release(_ ptr: UnsafeMutableRawPointer?) {}

@_cdecl("wk_web_extension_message_port_copy_application_identifier")
public func wk_web_extension_message_port_copy_application_identifier(_ ptr: UnsafeMutableRawPointer?) -> UnsafeMutablePointer<CChar>? { nil }

@_cdecl("wk_web_extension_message_port_is_disconnected")
public func wk_web_extension_message_port_is_disconnected(_ ptr: UnsafeMutableRawPointer?) -> Bool { true }

@_cdecl("wk_web_extension_message_port_send_message_json")
public func wk_web_extension_message_port_send_message_json(_ ptr: UnsafeMutableRawPointer?, _ messageJson: UnsafePointer<CChar>?, _ outErr: UnsafeMutablePointer<UnsafeMutablePointer<CChar>?>?) -> Int32 { WK_UNSUPPORTED }

@_cdecl("wk_web_extension_message_port_disconnect")
public func wk_web_extension_message_port_disconnect(_ ptr: UnsafeMutableRawPointer?) {}

@_cdecl("wk_web_extension_message_port_disconnect_with_error")
public func wk_web_extension_message_port_disconnect_with_error(_ ptr: UnsafeMutableRawPointer?, _ message: UnsafePointer<CChar>?) {}
