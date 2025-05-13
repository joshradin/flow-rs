use flow_rs::task_ordering::TaskOrderer;
use flow_rs::{DependsOn, Flow, FlowError, TaskId, TaskRef};

static PROJECTS: &[&str] = &[
    "base",
    "base-logic",
    "api",
    "core",
    "frontend",
    "root",
    "module1",
    "module2",
    "module3",
    "module4",
];
static PROJECT_DEPENDENCIES: &[(&str, &[&str])] = &[
    ("base", &["base-logic"]),
    ("base-logic", &["core"]),
    ("api", &["root", "base"]),
    ("frontend", &["api"]),
    ("module1", &["api"]),
    ("module2", &["api"]),
    ("module3", &["api"]),
    ("module4", &["api"]),
];

fn source_set(
    project: &str,
    set: &str,
    flow: &mut Flow<(), (), impl TaskOrderer>,
) -> Result<TaskId, FlowError> {
    let compile = flow.create(format!("{project}:{set}:compile"), || {
        // thread::sleep(Duration::from_millis(1500));
    });
    let p_r = flow.create(format!("{project}:{set}:processResources"), || {
        // thread::sleep(Duration::from_millis(150));
    });
    let mut jar = flow.create(format!("{project}:{set}:jar"), || {
        // thread::sleep(Duration::from_millis(210));
    });
    jar.depends_on(&compile)?;
    jar.depends_on(&p_r)?;
    Ok(*jar.id())
}

fn project(name: &str, flow: &mut Flow<(), (), impl TaskOrderer>) -> Result<(), FlowError> {
    let check = flow.create(format!("{name}:check"), || {});
    let mut assemble = flow.create(format!("{name}:assemble"), || {});
    let mut build = flow.create(format!("{name}:build"), || {});
    build.depends_on(&assemble)?;
    build.depends_on(&check)?;

    let main_jar = source_set(name, "main", flow)?;
    assemble.depends_on(main_jar)?;

    Ok(())
}

pub fn create_flow<TO: TaskOrderer>(flow: &mut Flow<(), (), TO>) -> Result<(), FlowError> {
    for project_name in PROJECTS {
        project(project_name, flow)?;
    }
    for (project_name, project_dependencies) in PROJECT_DEPENDENCIES {
        let this = flow.find(format!("{project_name}:main:compile")).unwrap();
        for &dep in *project_dependencies {
            let other = flow.find(format!("{dep}:main:jar")).unwrap();
            flow.depends_on(this, other)?;
        }
    }

    Ok(())
}

