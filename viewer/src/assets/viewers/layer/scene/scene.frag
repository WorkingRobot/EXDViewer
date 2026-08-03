#version 300 es
precision highp float;
precision highp sampler2D;

in vec3 v_normal;
in vec2 v_uv;
in float v_depth;

uniform sampler2D u_diffuse_map;
uniform bool u_have_diffuse;
uniform vec3 u_diffuse_color;
uniform vec3 u_emissive_color;
uniform float u_alpha_threshold;
uniform vec3 u_key;
uniform vec3 u_horizon;
uniform vec2 u_fade;

out vec4 fragColor;

void main() {
	vec4 albedo = vec4(u_diffuse_color, 1.0);
	if (u_have_diffuse) {
		albedo *= texture(u_diffuse_map, v_uv);
	}
	if (albedo.a < u_alpha_threshold) {
		discard;
	}

	vec3 normal = normalize(v_normal);
	if (!gl_FrontFacing) {
		normal = -normal;
	}
	float key = max(dot(normal, u_key), 0.0);
	float sky = 0.5 + 0.5 * normal.y;
	vec3 lit = albedo.rgb * (0.45 * sky + 0.7 * key) + u_emissive_color;

	// Everything past the load radius is missing rather than empty, so the last stretch before it
	// is faded into the background instead of ending at a wall.
	float fade = clamp((v_depth - u_fade.x) / max(u_fade.y - u_fade.x, 0.001), 0.0, 1.0);
	fragColor = vec4(mix(lit, u_horizon, fade), 1.0);
}
