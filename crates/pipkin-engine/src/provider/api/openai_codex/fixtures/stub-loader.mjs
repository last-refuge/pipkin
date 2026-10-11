// Resolve hook: bare npm specifiers (not installed) resolve to an inert stub module.
const STUB = "data:text/javascript," + encodeURIComponent(
	"const f = () => ({}); export const parse = f; export const Allow = { ALL: 0 }; export default {};",
);
export async function resolve(specifier, context, next) {
	if (!specifier.startsWith(".") && !specifier.startsWith("/") && !specifier.startsWith("node:") && !specifier.startsWith("file:") && !specifier.startsWith("data:")) {
		return { url: STUB, shortCircuit: true };
	}
	return next(specifier, context);
}
