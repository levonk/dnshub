# arg-parse-helpers.sh — shared helpers for shell argument parsers
#
# Provides:
#   reject_unknown_flag "$1"
#     Exit 1 with "Unknown option: $1" on stderr if $1 starts with `-`.
#     Call this as the first line of a `*)` default branch in a while/case
#     arg parser. Prevents unknown flags (--foo) from being silently
#     captured as positional arguments.
#
#   parse_value_flag "$1" "$2" out_var
#     Normalize a value-taking flag into out_var. Handles all three forms:
#       --flag value       (consumes 2 args)
#       --flag=value       (consumes 1 arg)
#       -f value           (consumes 2 args)
#       -f=value           (consumes 1 arg)
#     Sets out_var to the value and the global SHIFT_COUNT to 1 or 2.
#     Returns 0 on success, 1 if the flag does not match any recognized
#     form (caller should fall through to the next case).
#
#   parse_bool_flag "$1" var_name
#     Normalize a boolean flag into var_name. Handles:
#       --flag             (sets var_name=true, consumes 1 arg)
#       --flag=true        (sets var_name=true, consumes 1 arg)
#       --flag=false       (sets var_name=false, consumes 1 arg)
#       --flag=bogus       (exits 1 with error on stderr)
#     Returns 0 if the flag matched (var_name set), 1 if not (caller
#     falls through). SHIFT_COUNT is set to 1.
#
#   parse_format_flag "$1" "$2" out_var
#     Specialization of parse_value_flag for --format/-f/--json. Sets
#     out_var to the format value and SHIFT_COUNT to the shift count.
#     Recognizes:
#       --format value | --format=value | -f value | -f=value | --json
#     Returns 0 on match, 1 otherwise.
#
# Globals set by the parse_* helpers:
#   SHIFT_COUNT — number of args to shift after a successful parse
#
# Usage pattern in a script's main() arg parser:
#
#   source "$SCRIPT_DIR/arg-parse-helpers.sh"
#
#   while [[ $# -gt 0 ]]; do
#       case "$1" in
#       -v|--verbose)
#           parse_bool_flag "$1" VERBOSE; shift "$SHIFT_COUNT" ;;
#       -f|--format|--format=*|-f=*|--json)
#           if parse_format_flag "$1" "$2" format; then
#               shift "$SHIFT_COUNT"
#           else
#               reject_unknown_flag "$1"; shift
#           fi
#           ;;
#       -p|--path)
#           parse_value_flag "$1" "$2" repo_path; shift "$SHIFT_COUNT" ;;
#       --output|--output=*)
#           parse_value_flag "$1" "$2" output_path; shift "$SHIFT_COUNT" ;;
#       -h|--help)
#           usage; exit 0 ;;
#       *)
#           reject_unknown_flag "$1"
#           # ... positional assignment ...
#           shift
#           ;;
#       esac
#   done
#
# Materialization: each consuming skill has a
# `scripts/arg-parse-helpers.sh.tmpl` file containing a single include
# directive that pulls in this file. The templater inlines this file at
# build time. Scripts then `source` the materialized copy from the same
# `scripts/` directory.
#
# Consumers:
#   - project-detection/scripts/detect-all-systems.sh
#   - project-detection/scripts/detect-build-systems.sh
#   - project-detection/scripts/detect-ci-cd-systems.sh
#   - monorepo-extractor/scripts/detect-build-systems.sh
#   - monorepo-extractor/scripts/detect-ci-cd-systems.sh
#   - git-repository-management/scripts/git-tag.sh
#   - nixify/scripts/detect-garnix-scope.sh
#   - nixify/scripts/test-with-act.sh
#   - code-quality-validation/scripts/quality-validator.sh

# Guard against double-sourcing
if [[ -n "${_ARG_PARSE_HELPERS_SOURCED:-}" ]]; then
	return 0 2>/dev/null || exit 0
fi
_ARG_PARSE_HELPERS_SOURCED=1

# Global shift count set by parse_* helpers
SHIFT_COUNT=0

reject_unknown_flag() {
	if [[ "$1" == -* ]]; then
		echo "Unknown option: $1" >&2
		exit 1
	fi
}

# parse_value_flag "$1" "$2" out_var
# Returns 0 on match (out_var set, SHIFT_COUNT set), 1 on no match.
parse_value_flag() {
	local flag="$1"
	local next="$2"
	local out_var="$3"

	case "$flag" in
	--*=*)
		# --flag=value form
		printf -v "$out_var" '%s' "${flag#*=}"
		SHIFT_COUNT=1
		return 0
		;;
	-*=*)
		# -f=value form
		printf -v "$out_var" '%s' "${flag#*=}"
		SHIFT_COUNT=1
		return 0
		;;
	--*)
		# --flag value form (long flag, value is next arg)
		if [[ -z "$next" ]]; then
			echo "Option requires a value: $flag" >&2
			exit 1
		fi
		printf -v "$out_var" '%s' "$next"
		SHIFT_COUNT=2
		return 0
		;;
	-*)
		# -f value form (short flag, value is next arg)
		if [[ -z "$next" ]]; then
			echo "Option requires a value: $flag" >&2
			exit 1
		fi
		printf -v "$out_var" '%s' "$next"
		SHIFT_COUNT=2
		return 0
		;;
	*)
		# Not a value flag — caller should fall through
		return 1
		;;
	esac
}

# parse_bool_flag "$1" var_name
# Returns 0 on match (var_name set, SHIFT_COUNT=1), 1 on no match.
parse_bool_flag() {
	local flag="$1"
	local var_name="$2"

	case "$flag" in
	--*=*)
		# --flag=value form — validate the value
		local value="${flag#*=}"
		if [[ "$value" != "true" && "$value" != "false" ]]; then
			echo "Invalid boolean value for $flag: $value (expected true|false)" >&2
			exit 1
		fi
		printf -v "$var_name" '%s' "$value"
		SHIFT_COUNT=1
		return 0
		;;
	--*)
		# --flag form — implicit true
		printf -v "$var_name" '%s' "true"
		SHIFT_COUNT=1
		return 0
		;;
	*)
		# Not a boolean flag — caller should fall through
		return 1
		;;
	esac
}

# parse_format_flag "$1" "$2" out_var
# Specialization of parse_value_flag that also handles --json shorthand.
# Returns 0 on match (out_var set, SHIFT_COUNT set), 1 on no match.
parse_format_flag() {
	local flag="$1"
	local next="$2"
	local out_var="$3"

	# --json shorthand — always maps to "json"
	if [[ "$flag" == "--json" ]]; then
		printf -v "$out_var" '%s' "json"
		SHIFT_COUNT=1
		return 0
	fi

	# Delegate to parse_value_flag for --format/-f forms
	case "$flag" in
	--format | --format=* | -f | -f=*)
		parse_value_flag "$flag" "$next" "$out_var"
		return $?
		;;
	*)
		return 1
		;;
	esac
}
