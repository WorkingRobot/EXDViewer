#version 300 es
precision highp float;
precision highp int;
precision highp sampler2D;

in vec2 v_uv;
in vec4 v_color;

uniform sampler2D u_map;
uniform bool u_textured;
/// The blend the particle is drawn under, as `Blend` discriminates it.
uniform int u_mode;

out vec4 o_color;

void main() {
	vec4 color = v_color;
	if (u_textured) {
		color *= texture(u_map, v_uv);
	}
	if (u_mode == 0) {
		o_color = vec4(color.rgb, 1.0);
	} else if (u_mode == 2) {
		// Multiply takes the source straight, so opacity has to fade it towards white here rather
		// than through a blend factor.
		o_color = vec4(mix(vec3(1.0), color.rgb, color.a), 1.0);
	} else {
		o_color = vec4(color.rgb * color.a, color.a);
	}
}
